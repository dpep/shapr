//! Ruby's spelling variants: syntax with several spellings of one program,
//! and the canonical form each spelling reduces to for `restyle`
//! (docs/refactorings.md, section 3). A variant is named by the RuboCop cop
//! that chooses among its spellings; a norm with the same name judges which
//! way a restyle goes, so norms take their ids from here.
//!
//! A canonical form holds everything the node means. Where prism resolves
//! a spelling to a different tree (`do…end` binding to an outer call,
//! `and` binding looser than `=`), the trees differ and no form hides it.

use crate::tree::Canonical;
use ruby_prism::{Node, Visit};
use std::collections::HashMap;

pub const STRING_LITERALS: &str = "Style/StringLiterals";
pub const HASH_SYNTAX: &str = "Style/HashSyntax";
pub const QUOTED_SYMBOLS: &str = "Style/QuotedSymbols";
pub const WORD_ARRAY: &str = "Style/WordArray";
pub const SYMBOL_ARRAY: &str = "Style/SymbolArray";
pub const NUMERIC_LITERALS: &str = "Style/NumericLiterals";
pub const TRAILING_COMMA_ARRAY: &str = "Style/TrailingCommaInArrayLiteral";
pub const TRAILING_COMMA_HASH: &str = "Style/TrailingCommaInHashLiteral";
pub const TRAILING_COMMA_ARGUMENTS: &str = "Style/TrailingCommaInArguments";
pub const REDUNDANT_PARENTHESES: &str = "Style/RedundantParentheses";
pub const METHOD_CALL_PARENTHESES: &str = "Style/MethodCallWithArgsParentheses";
pub const COLON_METHOD_CALL: &str = "Style/ColonMethodCall";
pub const LAMBDA_CALL: &str = "Style/LambdaCall";
pub const NOT: &str = "Style/Not";
pub const BLOCK_DELIMITERS: &str = "Style/BlockDelimiters";
pub const AND_OR: &str = "Style/AndOr";
pub const IF_UNLESS_MODIFIER: &str = "Style/IfUnlessModifier";
pub const ONE_LINE_CONDITIONAL: &str = "Style/OneLineConditional";
pub const WHILE_UNTIL_MODIFIER: &str = "Style/WhileUntilModifier";
pub const DEF_PARENTHESES: &str = "Style/DefWithParentheses";
pub const ENDLESS_METHOD: &str = "Style/EndlessMethod";
pub const EMPTY_METHOD: &str = "Style/EmptyMethod";
pub const SINGLE_LINE_METHODS: &str = "Style/SingleLineMethods";
pub const SEMICOLON: &str = "Style/Semicolon";
pub const SUPER_ARGUMENTS: &str = "Style/SuperArguments";

/// The enclosing prism node, as (kind, start offset).
pub(super) type Parent = Option<(&'static str, usize)>;

/// What respelling a node needs from the code around it, gathered as the
/// walk opens a `def`.
#[derive(Default)]
pub(super) struct Context {
    /// The `super` calls that pass the enclosing method's parameters as
    /// they are, by start offset, each with its canonical form.
    forwarding: HashMap<usize, Vec<u8>>,
}

/// `node`'s variant and canonical form, when its syntax has other
/// spellings of the same program.
pub(super) fn respell(
    node: &Node<'_>,
    parent: Parent,
    source: &[u8],
    context: &mut Context,
) -> Option<(&'static str, Canonical)> {
    let parent_kind = parent.map(|(kind, _)| kind);
    let syntax = |variant| Some((variant, Canonical::Syntax(Vec::new())));
    if let Some(def) = node.as_def_node() {
        forwarding(&def, &mut context.forwarding);
    }
    if node.as_super_node().is_some() || node.as_forwarding_super_node().is_some() {
        let start = node.location().start_offset();
        let form = context.forwarding.get(&start)?;
        return Some((SUPER_ARGUMENTS, Canonical::Value(form.clone())));
    }
    if let Some(n) = node.as_string_node() {
        // A regexp's or command's text is read by the regexp engine or the
        // shell, escapes and all.
        if matches!(
            parent_kind,
            Some(
                "interpolated_regular_expression"
                    | "interpolated_match_last_line"
                    | "interpolated_x_string"
            )
        ) {
            return None;
        }
        // A `<<~` heredoc strips the indentation all its lines share, so a
        // part's value depends on lines outside it.
        let squiggly = parent.is_some_and(|(_, at)| source[at..].starts_with(b"<<~"));
        if parent_kind == Some("interpolated_string") && squiggly {
            return None;
        }
        let flags = [
            n.is_forced_utf8_encoding(),
            n.is_forced_binary_encoding(),
            n.is_frozen(),
            n.is_mutable(),
        ];
        let variant = match (n.opening_loc(), parent_kind) {
            (None, Some("array")) => WORD_ARRAY,
            _ => STRING_LITERALS,
        };
        return Some((
            variant,
            Canonical::Value(value(b'"', &flags, n.unescaped())),
        ));
    }
    if let Some(n) = node.as_interpolated_string_node() {
        // A `<<~` heredoc's parts are read together: each line loses the
        // indentation all of them share.
        let squiggly = n
            .opening_loc()
            .is_some_and(|o| o.as_slice().starts_with(b"<<~"));
        if !squiggly {
            return None;
        }
        let mut text = vec![u8::from(n.is_frozen()), u8::from(n.is_mutable())];
        for part in n.parts().iter() {
            match part.as_string_node() {
                Some(s) => {
                    let flags = [s.is_forced_utf8_encoding(), s.is_forced_binary_encoding()];
                    let part = value(b'"', &flags, s.unescaped());
                    text.push(b's');
                    text.extend((part.len() as u64).to_le_bytes());
                    text.extend(part);
                }
                None => text.push(b'e'),
            }
        }
        return Some((STRING_LITERALS, Canonical::Literal(text)));
    }
    if let Some(n) = node.as_symbol_node() {
        let flags = [
            n.is_forced_utf8_encoding(),
            n.is_forced_binary_encoding(),
            n.is_forced_us_ascii_encoding(),
        ];
        let start = n.location().start_offset();
        let variant = match (n.opening_loc(), parent) {
            (_, Some(("assoc", at))) if at == start => HASH_SYNTAX,
            (None, Some(("array", _))) => SYMBOL_ARRAY,
            _ => QUOTED_SYMBOLS,
        };
        return Some((
            variant,
            Canonical::Value(value(b':', &flags, n.unescaped())),
        ));
    }
    if let Some(n) = node.as_integer_node() {
        let integer = n.value();
        let (negative, digits) = integer.to_u32_digits();
        let mut bytes = vec![b'i', u8::from(negative)];
        let len = digits.iter().rposition(|&d| d != 0).map_or(0, |i| i + 1);
        bytes.extend(digits[..len].iter().flat_map(|d| d.to_le_bytes()));
        return Some((NUMERIC_LITERALS, Canonical::Value(bytes)));
    }
    if let Some(n) = node.as_float_node() {
        let mut bytes = vec![b'f'];
        bytes.extend(n.value().to_bits().to_le_bytes());
        return Some((NUMERIC_LITERALS, Canonical::Value(bytes)));
    }
    if let Some(n) = node.as_parentheses_node() {
        // `defined?` reports on the expression it is given, parentheses
        // included.
        let single = n
            .body()
            .and_then(|b| b.as_statements_node())
            .is_some_and(|s| s.body().iter().count() == 1);
        return (single && parent_kind != Some("defined"))
            .then_some((REDUNDANT_PARENTHESES, Canonical::Transparent));
    }
    if node.as_statements_node().is_some() {
        return match parent_kind {
            Some("parentheses") => Some((REDUNDANT_PARENTHESES, Canonical::Transparent)),
            _ => syntax(SEMICOLON),
        };
    }
    if node.as_implicit_node().is_some() {
        return Some((HASH_SYNTAX, Canonical::Transparent));
    }
    if let Some(n) = node.as_local_variable_read_node() {
        // Read by name and scope depth: `{x:}` spells the read as its label.
        let mut bytes = vec![b'l'];
        bytes.extend_from_slice(n.name().as_slice());
        bytes.push(0);
        bytes.extend(n.depth().to_le_bytes());
        return Some((HASH_SYNTAX, Canonical::Value(bytes)));
    }
    if node.as_assoc_node().is_some() {
        return syntax(HASH_SYNTAX);
    }
    if node.as_hash_node().is_some() {
        return syntax(TRAILING_COMMA_HASH);
    }
    if node.as_keyword_hash_node().is_some() || node.as_arguments_node().is_some() {
        return syntax(TRAILING_COMMA_ARGUMENTS);
    }
    if let Some(n) = node.as_array_node() {
        let opening = n.opening_loc().map(|o| o.as_slice().to_vec());
        return syntax(match opening.as_deref() {
            Some([b'%', b'w' | b'W', ..]) => WORD_ARRAY,
            Some([b'%', b'i' | b'I', ..]) => SYMBOL_ARRAY,
            _ => TRAILING_COMMA_ARRAY,
        });
    }
    if let Some(n) = node.as_call_node() {
        let mut text = n.name().as_slice().to_vec();
        text.push(0);
        text.extend(
            [
                n.is_safe_navigation(),
                n.is_variable_call(),
                n.is_attribute_write(),
                n.is_ignore_visibility(),
            ]
            .map(u8::from),
        );
        let message = n.message_loc().map(|m| m.as_slice().to_vec());
        let operator = n.call_operator_loc().map(|o| o.as_slice().to_vec());
        let variant = if message.as_deref() == Some(b"not") {
            NOT
        } else if operator.as_deref() == Some(b"::") {
            COLON_METHOD_CALL
        } else if message.is_none() && n.name().as_slice() == b"call" {
            LAMBDA_CALL
        } else if trailing_comma(&n, source) {
            TRAILING_COMMA_ARGUMENTS
        } else {
            METHOD_CALL_PARENTHESES
        };
        return Some((variant, Canonical::Syntax(text)));
    }
    if node.as_block_node().is_some() {
        return syntax(BLOCK_DELIMITERS);
    }
    if node.as_and_node().is_some() || node.as_or_node().is_some() {
        return syntax(AND_OR);
    }
    if let Some(n) = node.as_if_node() {
        return syntax(match n.if_keyword_loc() {
            None => ONE_LINE_CONDITIONAL,
            Some(_) => IF_UNLESS_MODIFIER,
        });
    }
    if node.as_unless_node().is_some() {
        return syntax(IF_UNLESS_MODIFIER);
    }
    if let Some(n) = node.as_else_node() {
        return match parent_kind {
            Some("if" | "unless") if n.else_keyword_loc().as_slice() == b":" => {
                syntax(ONE_LINE_CONDITIONAL)
            }
            Some("if" | "unless") => syntax(IF_UNLESS_MODIFIER),
            _ => None,
        };
    }
    if let Some(n) = node.as_while_node() {
        let form = vec![u8::from(n.is_begin_modifier())];
        return Some((WHILE_UNTIL_MODIFIER, Canonical::Syntax(form)));
    }
    if let Some(n) = node.as_until_node() {
        let form = vec![u8::from(n.is_begin_modifier())];
        return Some((WHILE_UNTIL_MODIFIER, Canonical::Syntax(form)));
    }
    if let Some(n) = node.as_def_node() {
        let variant = if n.equal_loc().is_some() {
            ENDLESS_METHOD
        } else if n.lparen_loc().is_some() && n.parameters().is_none() {
            DEF_PARENTHESES
        } else if n.body().is_none() {
            EMPTY_METHOD
        } else {
            SINGLE_LINE_METHODS
        };
        return Some((variant, Canonical::Syntax(n.name().as_slice().to_vec())));
    }
    None
}

/// Records the `super` calls in `def`'s own body (not a block's, where a
/// block parameter may shadow a method's) that mean what a bare `super`
/// means there: a bare `super` passes each parameter's current value, in
/// order, and the method's block, as `super(a, b)` does where `a, b` are
/// every parameter. Each gets the method's parameters as its form, so a
/// bare `super` under other parameters differs. Only required and optional
/// positionals and a block parameter are read; a method with any other
/// parameter records nothing.
fn forwarding(def: &ruby_prism::DefNode<'_>, out: &mut HashMap<usize, Vec<u8>>) {
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut form = b"zsuper".to_vec();
    if let Some(params) = def.parameters() {
        let plain = params.rest().is_none()
            && params.posts().iter().next().is_none()
            && params.keywords().iter().next().is_none()
            && params.keyword_rest().is_none();
        if !plain {
            return;
        }
        for (tag, list) in [(b'r', params.requireds()), (b'o', params.optionals())] {
            for p in list.iter() {
                let name = if let Some(r) = p.as_required_parameter_node() {
                    r.name().as_slice().to_vec()
                } else if let Some(o) = p.as_optional_parameter_node() {
                    o.name().as_slice().to_vec()
                } else {
                    return; // a destructuring `(a, b)`
                };
                form.push(tag);
                form.extend_from_slice(&name);
                form.push(0);
                names.push(name);
            }
        }
        if params.block().is_some() {
            form.push(b'&');
        }
    }
    let Some(body) = def.body() else {
        return;
    };
    let mut scan = Supers {
        names: &names,
        form: &form,
        out,
    };
    scan.visit(&body);
}

/// The walk behind [`forwarding`], over a method's own scope.
struct Supers<'s> {
    names: &'s [Vec<u8>],
    form: &'s [u8],
    out: &'s mut HashMap<usize, Vec<u8>>,
}

impl<'pr> Visit<'pr> for Supers<'_> {
    fn visit_super_node(&mut self, node: &ruby_prism::SuperNode<'pr>) {
        let arguments: Vec<Node<'_>> = node
            .arguments()
            .map(|a| a.arguments().iter().collect())
            .unwrap_or_default();
        let passed = arguments.len() == self.names.len()
            && arguments.iter().zip(self.names).all(|(arg, name)| {
                arg.as_local_variable_read_node()
                    .is_some_and(|r| r.depth() == 0 && r.name().as_slice() == name.as_slice())
            });
        if passed && node.block().is_none() {
            self.out
                .insert(node.location().start_offset(), self.form.to_vec());
        }
    }

    fn visit_forwarding_super_node(&mut self, node: &ruby_prism::ForwardingSuperNode<'pr>) {
        if node.block().is_none() {
            self.out
                .insert(node.location().start_offset(), self.form.to_vec());
        }
    }

    // A new scope: its locals, `self` or meaning of `super` are its own.
    fn visit_block_node(&mut self, _: &ruby_prism::BlockNode<'pr>) {}
    fn visit_lambda_node(&mut self, _: &ruby_prism::LambdaNode<'pr>) {}
    fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {}
    fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {}
    fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {}
    fn visit_singleton_class_node(&mut self, _: &ruby_prism::SingletonClassNode<'pr>) {}
}

/// A comma after a call's last argument, before its closing parenthesis.
fn trailing_comma(call: &ruby_prism::CallNode<'_>, source: &[u8]) -> bool {
    let (Some(arguments), Some(closing)) = (call.arguments(), call.closing_loc()) else {
        return false;
    };
    let gap = arguments.location().end_offset()..closing.start_offset();
    source.get(gap).is_some_and(|text| text.contains(&b','))
}

/// A literal's canonical form: its type, its flags and its bytes.
fn value(tag: u8, flags: &[bool], bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(flags.iter().map(|&f| u8::from(f)));
    out.extend_from_slice(bytes);
    out
}
