//! Which prism nodes play which [`Role`], and the names prism doesn't record
//! on the node itself.

use crate::tree::{Jump, NameKind, Receiver, Role, Visibility};
use ruby_prism::Node;
use std::collections::HashSet;

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

/// Remembers which literals are arguments to a dynamic call; prism visits a
/// call before its arguments.
#[derive(Default)]
pub(super) struct Roles {
    dynamic_args: HashSet<usize>,
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
            Node::GlobalVariableReadNode { .. } => Role::Reference(Global),
            Node::ConstantReadNode { .. } => Role::Reference(Constant),
            Node::ConstantPathNode { .. } => {
                let name = node.as_constant_path_node().and_then(|n| owned(n.name()));
                return (Role::Reference(Constant), name);
            }
            Node::CallNode { .. } => return (self.call(node), None),
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
            Node::ReturnNode { .. } => Role::Jump(Jump::Return),
            Node::BreakNode { .. } => Role::Jump(Jump::Break),
            Node::NextNode { .. } => Role::Jump(Jump::Next),
            Node::RedoNode { .. } => Role::Jump(Jump::Redo),
            Node::RetryNode { .. } => Role::Jump(Jump::Retry),
            Node::YieldNode { .. } => Role::Jump(Jump::Yield),
            Node::SuperNode { .. } | Node::ForwardingSuperNode { .. } => Role::Jump(Jump::Super),
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
            Some(r) if r.as_self_node().is_some() => Receiver::SelfRef,
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
        if dynamic {
            for arg in call.arguments().iter().flat_map(|a| a.arguments().iter()) {
                if arg.as_symbol_node().is_some() || arg.as_string_node().is_some() {
                    self.dynamic_args.insert(arg.location().start_offset());
                }
            }
        }
        match visibility {
            Some(v) => Role::Visibility(v),
            None if receiver == Receiver::Implicit
                && matches!(name, b"block_given?" | b"iterator?") =>
            {
                Role::Jump(Jump::BlockGiven)
            }
            None => Role::Call(receiver),
        }
    }
}
