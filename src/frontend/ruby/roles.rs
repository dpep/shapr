//! Which prism nodes play which [`Role`], and the names prism doesn't record
//! on the node itself.

use crate::frontend::Construct;
use crate::tree::{Flags, Hazard, NameKind, NodeId, Receiver, Role, Tree, Visibility};
use ruby_prism::Node;
use std::collections::{HashMap, HashSet};

/// Calls whose string or symbol arguments name code resolved at runtime.
const DYNAMIC: &[&str] = &[
    "send",
    "public_send",
    "__send__",
    "method",
    "define_method",
    "respond_to?",
    "instance_variable_get",
    "instance_variable_set",
    "const_get",
    "alias_method",
    "attr",
    "attr_reader",
    "attr_writer",
    "attr_accessor",
];

/// Dynamic calls whose first argument is the name of the method they reach.
const SENDERS: &[&str] = &[
    "send",
    "public_send",
    "__send__",
    "try",
    "method",
    "public_method",
    "instance_method",
    "define_method",
    "respond_to?",
];

/// Remembers which literals are arguments to a dynamic call, and the
/// argument lists of dynamic calls whose name is computed; prism visits a
/// call before its arguments.
#[derive(Default)]
pub(super) struct Roles {
    dynamic_args: HashSet<usize>,
    /// Argument lists, by start offset, with the pattern their computed
    /// name matches.
    computed: HashMap<usize, Vec<u8>>,
}

impl Roles {
    /// The node's role, and its name when prism keeps it elsewhere than
    /// `name()` (or the literal's value, for a dynamic name).
    pub(super) fn of(&mut self, node: &Node<'_>) -> (Role, Option<Vec<u8>>) {
        use NameKind::*;
        let owned = |name: Option<ruby_prism::ConstantId<'_>>| name.map(|n| n.as_slice().to_vec());
        let role = match node {
            Node::DefNode { .. } => Role::Definition(Method),
            Node::ClassNode { .. } => Role::Definition(Class),
            Node::ModuleNode { .. } => Role::Definition(Module),
            Node::ConstantWriteNode { .. }
            | Node::ConstantOperatorWriteNode { .. }
            | Node::ConstantAndWriteNode { .. }
            | Node::ConstantOrWriteNode { .. }
            | Node::ConstantTargetNode { .. } => Role::Definition(Constant),
            Node::ConstantPathWriteNode { .. } => {
                let target = node.as_constant_path_write_node().map(|n| n.target());
                return (
                    Role::Definition(Constant),
                    target.and_then(|t| owned(t.name())),
                );
            }
            Node::LocalVariableWriteNode { .. } => Role::Bind,
            Node::LocalVariableOperatorWriteNode { .. }
            | Node::LocalVariableAndWriteNode { .. }
            | Node::LocalVariableOrWriteNode { .. }
            | Node::LocalVariableTargetNode { .. }
            | Node::BlockLocalVariableNode { .. } => Role::Definition(Variable),
            Node::InstanceVariableWriteNode { .. }
            | Node::InstanceVariableOperatorWriteNode { .. }
            | Node::InstanceVariableAndWriteNode { .. }
            | Node::InstanceVariableOrWriteNode { .. }
            | Node::InstanceVariableTargetNode { .. }
            | Node::ClassVariableWriteNode { .. }
            | Node::ClassVariableOperatorWriteNode { .. }
            | Node::ClassVariableAndWriteNode { .. }
            | Node::ClassVariableOrWriteNode { .. }
            | Node::ClassVariableTargetNode { .. } => Role::Definition(Field),
            Node::GlobalVariableWriteNode { .. }
            | Node::GlobalVariableOperatorWriteNode { .. }
            | Node::GlobalVariableAndWriteNode { .. }
            | Node::GlobalVariableOrWriteNode { .. }
            | Node::GlobalVariableTargetNode { .. } => Role::Definition(Global),
            Node::RequiredParameterNode { .. } | Node::OptionalParameterNode { .. } => {
                Role::Definition(Parameter)
            }
            Node::RequiredKeywordParameterNode { .. }
            | Node::OptionalKeywordParameterNode { .. } => Role::Definition(KeywordParameter),
            Node::RestParameterNode { .. } => {
                let name = node.as_rest_parameter_node().and_then(|n| owned(n.name()));
                return (Role::Definition(Parameter), name);
            }
            Node::KeywordRestParameterNode { .. } => {
                let name = node
                    .as_keyword_rest_parameter_node()
                    .and_then(|n| owned(n.name()));
                return (Role::Definition(Parameter), name);
            }
            Node::BlockParameterNode { .. } => {
                let name = node.as_block_parameter_node().and_then(|n| owned(n.name()));
                return (Role::Definition(BlockParameter), name);
            }
            Node::LocalVariableReadNode { .. } => Role::Reference(Variable),
            Node::InstanceVariableReadNode { .. } | Node::ClassVariableReadNode { .. } => {
                Role::Reference(Field)
            }
            Node::GlobalVariableReadNode { .. } => {
                let name = node.as_global_variable_read_node().map(|n| n.name());
                // The last match and the last line read are local to the frame.
                match name.map(|n| n.as_slice()) {
                    Some(b"$~" | b"$_") => Role::Hazard(Hazard::Frame),
                    _ => Role::Reference(Global),
                }
            }
            Node::NumberedReferenceReadNode { .. } | Node::BackReferenceReadNode { .. } => {
                Role::Hazard(Hazard::Frame)
            }
            Node::SourceLineNode { .. } => Role::Hazard(Hazard::Line),
            Node::SourceFileNode { .. } => Role::Hazard(Hazard::File),
            Node::ConstantReadNode { .. } => Role::Reference(Constant),
            Node::ConstantPathNode { .. } => {
                let name = node.as_constant_path_node().and_then(|n| owned(n.name()));
                return (Role::Reference(Constant), name);
            }
            Node::CallNode { .. } => return (self.call(node), None),
            Node::ArgumentsNode { .. } => {
                return match self.computed.remove(&node.location().start_offset()) {
                    Some(pattern) => (Role::DynamicName, Some(pattern)),
                    None => (Role::Other, None),
                };
            }
            Node::AliasMethodNode { .. } => {
                if let Some(alias) = node.as_alias_method_node() {
                    self.dynamic_args
                        .insert(alias.new_name().location().start_offset());
                    self.dynamic_args
                        .insert(alias.old_name().location().start_offset());
                }
                Role::Other
            }
            Node::SymbolNode { .. } | Node::StringNode { .. } => {
                if !self.dynamic_args.remove(&node.location().start_offset()) {
                    return (Role::Other, None);
                }
                let value = match node.as_symbol_node() {
                    Some(symbol) => symbol.unescaped().to_vec(),
                    None => node
                        .as_string_node()
                        .map(|s| s.unescaped().to_vec())
                        .unwrap_or_default(),
                };
                return (Role::DynamicName, Some(value));
            }
            Node::ReturnNode { .. } => Role::Hazard(Hazard::Return),
            Node::BreakNode { .. } => Role::Hazard(Hazard::Break),
            Node::NextNode { .. } => Role::Hazard(Hazard::Next),
            Node::RedoNode { .. } => Role::Hazard(Hazard::Redo),
            Node::RetryNode { .. } => Role::Hazard(Hazard::Retry),
            Node::YieldNode { .. } => Role::Hazard(Hazard::Yield),
            Node::SuperNode { .. } | Node::ForwardingSuperNode { .. } => {
                Role::Hazard(Hazard::Super)
            }
            _ => Role::Other,
        };
        (role, None)
    }

    fn call(&mut self, node: &Node<'_>) -> Role {
        let Some(call) = node.as_call_node() else {
            return Role::Other;
        };
        let name = call.name().as_slice();
        let receiver = match call.receiver() {
            None => Receiver::Implicit,
            Some(r) if r.as_self_node().is_some() && names_its_message(&call) => Receiver::SelfRef,
            Some(_) => Receiver::Explicit,
        };
        let visibility = match name {
            b"public" => Some(Visibility::Public),
            b"protected" => Some(Visibility::Protected),
            b"private" => Some(Visibility::Private),
            b"module_function" => Some(Visibility::ModuleFunction),
            _ => None,
        }
        .filter(|_| receiver == Receiver::Implicit);
        let dynamic = visibility.is_some() || DYNAMIC.iter().any(|d| d.as_bytes() == name);
        if dynamic && let Some(args) = call.arguments() {
            let mut computed = Vec::new();
            for (i, arg) in args.arguments().iter().enumerate() {
                if arg.as_symbol_node().is_some() || arg.as_string_node().is_some() {
                    self.dynamic_args.insert(arg.location().start_offset());
                } else if let Some(pattern) = interpolated(&arg) {
                    computed.push(pattern);
                } else if i == 0 && SENDERS.iter().any(|s| s.as_bytes() == name) {
                    computed.push(b"*".to_vec());
                }
            }
            let pattern = match &computed[..] {
                [] => None,
                [one] => Some(one.clone()),
                _ => Some(b"*".to_vec()),
            };
            if let Some(pattern) = pattern {
                self.computed
                    .insert(args.location().start_offset(), pattern);
            }
        }
        if let Some(v) = visibility {
            return Role::Visibility(v);
        }
        let implicit = receiver == Receiver::Implicit;
        let blockless = call.block().is_none();
        let proc_new = name == b"new"
            && call
                .receiver()
                .and_then(|r| r.as_constant_read_node())
                .is_some_and(|c| c.name().as_slice() == b"Proc");
        match name {
            b"block_given?" | b"iterator?" if implicit => Role::Hazard(Hazard::BlockGiven),
            // With no block of its own, a proc takes the method's block.
            b"proc" | b"lambda" if implicit && blockless => Role::Hazard(Hazard::BlockGiven),
            _ if proc_new && blockless => Role::Hazard(Hazard::BlockGiven),
            b"binding" | b"local_variables" | b"eval" if implicit => Role::Hazard(Hazard::Binding),
            // A string evaluated in another self still sees the caller's locals.
            b"instance_eval" | b"class_eval" | b"module_eval" if call.arguments().is_some() => {
                Role::Hazard(Hazard::Binding)
            }
            b"__method__" | b"__callee__" | b"caller" | b"caller_locations" if implicit => {
                Role::Hazard(Hazard::Frame)
            }
            b"__dir__" | b"require_relative" if implicit => Role::Hazard(Hazard::File),
            _ => Role::Call(receiver),
        }
    }
}

/// What the core reads off a node beside its role: a method, class or
/// module body bounds its locals; `def self.x` (or `self::x`) has a
/// receiver; a call's `arguments` list.
pub(super) fn flags(node: &Node<'_>) -> Flags {
    match node {
        Node::DefNode { .. } => {
            let singleton = node.as_def_node().is_some_and(|d| d.receiver().is_some());
            match singleton {
                true => Flags::SCOPE | Flags::RECEIVER,
                false => Flags::SCOPE,
            }
        }
        Node::ClassNode { .. } | Node::ModuleNode { .. } => Flags::SCOPE,
        Node::ArgumentsNode { .. } => Flags::ARGUMENTS,
        // `{x:}`, `g(x:)`: the value read also names the key.
        Node::ImplicitNode { .. } => Flags::MEMBER,
        _ => Flags::default(),
    }
}

/// Whether a call on `self` spells the method it calls the plain way:
/// `self.name`, `self::name`, an operator (`self + x`) or an index
/// (`self[x]`). `self&.name`, `-self` (`-@`) and `self.()` don't.
fn names_its_message(call: &ruby_prism::CallNode<'_>) -> bool {
    let name = call.name().as_slice();
    let message = call.message_loc().map(|m| m.as_slice());
    match call.call_operator_loc().map(|o| o.as_slice()) {
        Some(b"." | b"::") => message.is_some(),
        Some(_) => false,
        None => message == Some(name) || name == b"[]",
    }
}

/// An interpolated string or symbol as a name pattern: its fixed text, `*`
/// for each interpolation (`"get_#{x}"` is `get_*`).
fn interpolated(node: &Node<'_>) -> Option<Vec<u8>> {
    let parts = match node {
        Node::InterpolatedStringNode { .. } => node.as_interpolated_string_node()?.parts(),
        Node::InterpolatedSymbolNode { .. } => node.as_interpolated_symbol_node()?.parts(),
        _ => return None,
    };
    let mut pattern = Vec::new();
    for part in parts.iter() {
        match part.as_string_node() {
            Some(text) => pattern.extend_from_slice(text.unescaped()),
            None if pattern.last() != Some(&b'*') => pattern.push(b'*'),
            None => {}
        }
    }
    Some(pattern)
}

/// Which prism nodes are which [`Construct`], by kind and name.
pub(super) fn construct(tree: &Tree, id: NodeId) -> Option<Construct> {
    let node = tree.node(id);
    let name = node.name.as_deref();
    Some(match node.kind {
        // What a conditional tests leads its children, before its bodies.
        "if" | "unless" | "while" | "until" | "case" | "case_match" | "when" | "in" => {
            let test = node
                .children
                .iter()
                .map(|&c| tree.node(c))
                .filter(|c| !c.comment)
                .take_while(|c| !matches!(c.kind, "statements" | "when" | "in" | "else"))
                .count();
            Construct::Branch { test }
        }
        "and" | "or" => Construct::Comparison,
        "call" if matches!(name, Some(n) if COMPARISONS.contains(&n)) => Construct::Comparison,
        "call"
            if matches!(name, Some("raise" | "fail"))
                && node.role == Role::Call(Receiver::Implicit) =>
        {
            Construct::Error
        }
        "rescue" | "rescue_modifier" | "ensure" => Construct::Error,
        "integer" | "float" | "rational" | "imaginary" | "string" | "symbol" | "true" | "false"
        | "nil" | "regular_expression" => Construct::Literal,
        "parameters" => Construct::Parameters,
        _ => return None,
    })
}

/// Methods that compare or negate.
const COMPARISONS: &[&str] = &[
    "==", "!=", "<", ">", "<=", ">=", "<=>", "===", "=~", "!~", "!", "eql?", "equal?",
];
