//! Naming/MethodName and Naming/VariableName: method and local variable
//! names in `snake_case` or `camelCase`. Only a name of two or more words
//! chooses: `total` reads the same in both.

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;
use std::ops::Range;

pub const METHOD: &str = "Naming/MethodName";
pub const VARIABLE: &str = "Naming/VariableName";
pub const STYLES: &[&str] = &["snake_case", "camelCase"];

/// What the walk gathers for the passes after it.
#[derive(Default)]
pub struct Names {
    /// Each `def`'s name, and whether a rename may be suggested: a private
    /// instance method.
    defs: Vec<(Range<usize>, bool)>,
    /// Name starts of `def`s in a `private` section or under `private def`.
    private: Vec<usize>,
    /// Messages of calls with no receiver or `self`.
    calls: Vec<Range<usize>>,
    /// Each local scope (program, class, method, block) and the locals it
    /// declares.
    scopes: Vec<(Range<usize>, Vec<Vec<u8>>)>,
    /// Every read, write and non-keyword parameter of a local, by its name.
    locals: Vec<Range<usize>>,
}

pub fn see_variables(w: &mut Walker, node: &Node) {
    let locals = if let Some(n) = node.as_program_node() {
        Some(n.locals())
    } else if let Some(n) = node.as_def_node() {
        Some(n.locals())
    } else if let Some(n) = node.as_block_node() {
        Some(n.locals())
    } else if let Some(n) = node.as_lambda_node() {
        Some(n.locals())
    } else if let Some(n) = node.as_class_node() {
        Some(n.locals())
    } else if let Some(n) = node.as_module_node() {
        Some(n.locals())
    } else {
        node.as_singleton_class_node().map(|n| n.locals())
    };
    if let Some(locals) = locals {
        let loc = node.location();
        let names = locals.iter().map(|c| c.as_slice().to_vec()).collect();
        w.names
            .scopes
            .push((loc.start_offset()..loc.end_offset(), names));
        return;
    }
    let name = if let Some(n) = node.as_local_variable_read_node() {
        let end = n.location().end_offset();
        // `{a:}` reads `a` through its key: renaming it would rename the key.
        if w.source.get(end) == Some(&b':') && w.source.get(end + 1) != Some(&b':') {
            return;
        }
        Some(n.location())
    } else if let Some(n) = node.as_local_variable_write_node() {
        Some(n.name_loc())
    } else if let Some(n) = node.as_local_variable_target_node() {
        Some(n.location())
    } else if let Some(n) = node.as_local_variable_operator_write_node() {
        Some(n.name_loc())
    } else if let Some(n) = node.as_local_variable_and_write_node() {
        Some(n.name_loc())
    } else if let Some(n) = node.as_local_variable_or_write_node() {
        Some(n.name_loc())
    } else if let Some(n) = node.as_required_parameter_node() {
        Some(n.location())
    } else if let Some(n) = node.as_optional_parameter_node() {
        Some(n.name_loc())
    } else if let Some(n) = node.as_rest_parameter_node() {
        n.name_loc()
    } else if let Some(n) = node.as_keyword_rest_parameter_node() {
        n.name_loc()
    } else if let Some(n) = node.as_block_parameter_node() {
        n.name_loc()
    } else {
        node.as_block_local_variable_node().map(|n| n.location())
    };
    if let Some(name) = name {
        w.names.locals.push(name.start_offset()..name.end_offset());
    }
}

pub fn see(w: &mut Walker, node: &Node) {
    if let Some(def) = node.as_def_node() {
        let name = def.name_loc();
        let instance = def.receiver().is_none();
        w.names
            .defs
            .push((name.start_offset()..name.end_offset(), instance));
    } else if let Some(call) = node.as_call_node() {
        if call.receiver().is_none_or(|r| r.as_self_node().is_some())
            && let Some(message) = call.message_loc()
        {
            w.names
                .calls
                .push(message.start_offset()..message.end_offset());
        }
    } else if let Some(statements) = body(node) {
        // `private` holds until `public` or `protected`.
        let mut private = false;
        for statement in statements.body().iter() {
            if let Some((name, def)) = modifier(&statement) {
                match def {
                    Some(start) if name == b"private" => w.names.private.push(start),
                    Some(_) => {}
                    None => private = name == b"private",
                }
            } else if let (true, Some(def)) = (private, statement.as_def_node()) {
                w.names.private.push(def.name_loc().start_offset());
            }
        }
    }
}

/// A class, module or singleton class body.
fn body<'pr>(node: &Node<'pr>) -> Option<ruby_prism::StatementsNode<'pr>> {
    let body = if let Some(class) = node.as_class_node() {
        class.body()
    } else if let Some(module) = node.as_module_node() {
        module.body()
    } else if let Some(singleton) = node.as_singleton_class_node() {
        singleton.body()
    } else {
        None
    };
    body?.as_statements_node()
}

/// A visibility call with no receiver: its name, and the name start of a
/// `def` it takes (`private def x`).
fn modifier<'a>(node: &Node<'a>) -> Option<(&'a [u8], Option<usize>)> {
    let call = node.as_call_node()?;
    let name = call.name().as_slice();
    if call.receiver().is_some()
        || call.block().is_some()
        || !matches!(name, b"private" | b"protected" | b"public")
    {
        return None;
    }
    let Some(arguments) = call.arguments() else {
        return Some((name, None));
    };
    let mut arguments = arguments.arguments().iter();
    let def = match (arguments.next(), arguments.next()) {
        (Some(only), None) => only.as_def_node().map(|d| d.name_loc().start_offset()),
        _ => None,
    };
    def.map(|start| (name, Some(start)))
}

/// Each method name that chooses, after the walk. A rename is suggested
/// only for a private instance method whose every mention in the file is its
/// `def` or a call by the same name with no receiver or `self`; other
/// methods are interface.
pub fn methods(w: &mut Walker) {
    let source = w.source;
    let defs = std::mem::take(&mut w.names.defs);
    for (span, instance) in defs {
        let name = &source[span.clone()];
        let Some((style, core)) = case(name) else {
            continue;
        };
        let fix = (instance && w.names.private.contains(&span.start))
            .then(|| {
                let calls = (w.names.calls.iter()).filter(|c| &source[(*c).clone()] == name);
                let uses: Vec<usize> = std::iter::once(span.start)
                    .chain(calls.map(|c| c.start))
                    .collect();
                rename(source, name, 0..source.len(), &uses)
            })
            .flatten();
        let line = w.index.line(span.start);
        w.push(
            METHOD,
            style,
            span.start,
            (line, line),
            snake(core),
            fix.into_iter().collect(),
        );
    }
}

/// Each local that chooses, once per scope that declares it, at its first
/// mention. A rename covers the scope, and is suggested when every mention
/// there is the local, and nothing in the scope can read locals by name
/// (`binding`, `eval`, `local_variables`). A keyword parameter is interface:
/// its mention blocks the rename.
pub fn variables(w: &mut Walker) {
    let source = w.source;
    let scopes = std::mem::take(&mut w.names.scopes);
    for (range, names) in scopes {
        let mut dynamic = None;
        for name in names {
            let Some((style, core)) = case(&name) else {
                continue;
            };
            let uses: Vec<usize> = (w.names.locals.iter())
                .filter(|l| range.contains(&l.start) && source[(*l).clone()] == name[..])
                .map(|l| l.start)
                .collect();
            let Some(&first) = uses.iter().min() else {
                continue;
            };
            let dynamic = *dynamic.get_or_insert_with(|| {
                [&b"binding"[..], b"eval", b"local_variables"]
                    .iter()
                    .any(|word| !mentions(source, range.clone(), word).is_empty())
            });
            let fix = (!dynamic)
                .then(|| rename(source, &name, range.clone(), &uses))
                .flatten();
            let line = w.index.line(first);
            w.push(
                VARIABLE,
                style,
                first,
                (line, line),
                snake(core),
                fix.into_iter().collect(),
            );
        }
    }
}

/// `name` respelled in the other style everywhere `range` mentions it, when
/// each mention starts at one of `uses` and the new name is unused there.
fn rename(
    source: &[u8],
    name: &[u8],
    range: Range<usize>,
    uses: &[usize],
) -> Option<(String, Vec<Replace>)> {
    let (style, core) = case(name)?;
    let (other, text) = if style == "snake_case" {
        ("camelCase", camel(core)?)
    } else {
        ("snake_case", snake(core))
    };
    if case(text.as_bytes()).map(|(s, _)| s) != Some(other) {
        return None;
    }
    // The name as a whole identifier: its leading `_`, not its suffix.
    let prefix = core.as_ptr() as usize - name.as_ptr() as usize;
    let word = &name[..prefix + core.len()];
    let new_word = [&word[..prefix], text.as_bytes()].concat();
    if !mentions(source, range.clone(), &new_word).is_empty() {
        return None;
    }
    let found = mentions(source, range, word);
    found.iter().all(|at| uses.contains(at)).then(|| {
        let replaces = found
            .iter()
            .map(|&at| Replace {
                span: at + prefix..at + word.len(),
                text: text.clone(),
            })
            .collect();
        (other.to_string(), replaces)
    })
}

/// The style a name chooses and its words without a leading `_` or a
/// trailing `?`, `!` or `=`: `snake_case` for lowercase words joined by
/// `_`, `camelCase` for a lowercase start with capitals and no `_`.
pub fn case(name: &[u8]) -> Option<(&'static str, &[u8])> {
    let core = name.strip_prefix(b"_").unwrap_or(name);
    let core = core
        .strip_suffix(b"?")
        .or_else(|| core.strip_suffix(b"!"))
        .or_else(|| core.strip_suffix(b"="))
        .unwrap_or(core);
    if !core.first()?.is_ascii_lowercase()
        || !core.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        return None;
    }
    let upper = core.iter().any(u8::is_ascii_uppercase);
    let words = core.split(|&b| b == b'_');
    let style = if upper {
        (!core.contains(&b'_')).then_some("camelCase")
    } else {
        let mut words = words.peekable();
        let first = words.next();
        (words.peek().is_some() && first.into_iter().chain(words).all(|w| !w.is_empty()))
            .then_some("snake_case")
    };
    style.map(|s| (s, core))
}

/// `fooBar` and `getHTTPCode` as `foo_bar` and `get_http_code`; a snake
/// name as itself.
pub fn snake(core: &[u8]) -> String {
    let mut out = String::new();
    for (i, &b) in core.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| core[p]);
        let next = core.get(i + 1);
        let boundary = b.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_ascii_uppercase() && next.is_some_and(u8::is_ascii_lowercase))
            });
        if boundary {
            out.push('_');
        }
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}

/// `foo_bar` as `fooBar`; none when a word after the first doesn't start
/// with a letter.
pub fn camel(core: &[u8]) -> Option<String> {
    let mut words = core.split(|&b| b == b'_');
    let mut out = String::from_utf8_lossy(words.next()?).into_owned();
    for word in words {
        let (first, rest) = word.split_first()?;
        if !first.is_ascii_lowercase() {
            return None;
        }
        out.push(first.to_ascii_uppercase() as char);
        out.push_str(&String::from_utf8_lossy(rest));
    }
    Some(out)
}

/// Starts of `word` in `range` as a whole identifier: not part of a longer
/// name, an `@ivar` or a `$global`. Symbols, strings and comments count.
pub fn mentions(source: &[u8], range: Range<usize>, word: &[u8]) -> Vec<usize> {
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let hay = &source[range.clone()];
    (0..hay.len().saturating_sub(word.len() - 1))
        .filter(|&i| hay[i..].starts_with(word))
        .map(|i| range.start + i)
        .filter(|&at| {
            let before = at.checked_sub(1).map(|p| source[p]);
            let after = source.get(at + word.len()).copied();
            !before.is_some_and(|b| ident(b) || matches!(b, b'@' | b'$'))
                && !after.is_some_and(ident)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::Ruby;
    use super::super::tests::styles;
    use super::*;
    use crate::norms::Language;

    #[test]
    fn names_of_two_or_more_words_choose() {
        let src = "def total; end\ndef line_items; end\ndef lineItems; end\ndef self.fetchAll; end\n\
                   def valid?; end\ndef _cache_key; end\ndef ==(o); end\ndef Foo; end\ndef a_B; end\n";
        assert_eq!(
            styles(src, METHOD),
            "snake_case@2 camelCase@3 camelCase@4 snake_case@6"
        );
    }

    #[test]
    fn locals_choose_once_per_scope() {
        let src = "itemCount = 1\ndef a(first_name, lastName, keyArg: 1)\n  totalSum = 0\n  \
                   totalSum += 1\n  [1].each { |rowIndex| rowIndex }\n  x = 2\nend\n";
        assert_eq!(
            styles(src, VARIABLE),
            "camelCase@1 snake_case@2 camelCase@2 camelCase@3 camelCase@5"
        );
    }

    /// The suggested rename for each `camelCase` local, as `line: text@line...`.
    fn local_renames(src: &str) -> Vec<String> {
        let analysis = Ruby.analyze(src.as_bytes());
        let index = crate::norms::LineIndex::new(src.as_bytes());
        (analysis.observations.iter())
            .filter(|o| o.norm == VARIABLE && o.style == "camelCase")
            .map(|o| {
                let fix = o.fix.first().map(|(_, replaces)| {
                    let at: Vec<String> = (replaces.iter())
                        .map(|r| format!("{}@{}", r.text, index.line(r.span.start)))
                        .collect();
                    at.join(" ")
                });
                format!("{}: {}", o.line, fix.unwrap_or_default())
            })
            .collect()
    }

    #[test]
    fn a_local_rename_covers_its_scope_unless_something_else_mentions_it() {
        let src = "def a(rowCount)\n  rowCount.times { |i| p rowCount }\nend\n\
                   def b\n  userName = 1\n  go(userName:)\nend\n\
                   def c\n  maxSize = 1\n  binding\nend\n\
                   def d\n  newName = 1\n  new_name = 2\nend\n";
        assert_eq!(
            local_renames(src),
            [
                "1: row_count@1 row_count@2 row_count@2",
                "5: ",
                "9: ",
                "13: "
            ]
        );
    }

    #[test]
    fn spellings_convert_both_ways() {
        assert_eq!(snake(b"getHTTPCode"), "get_http_code");
        assert_eq!(snake(b"fooBar2"), "foo_bar2");
        assert_eq!(camel(b"line_items").as_deref(), Some("lineItems"));
        assert_eq!(camel(b"item_2"), None);
    }

    /// The suggested rename for each `camelCase` method, as `name: text@line...`.
    fn renames(src: &str) -> Vec<String> {
        let analysis = Ruby.analyze(src.as_bytes());
        let index = crate::norms::LineIndex::new(src.as_bytes());
        (analysis.observations.iter())
            .filter(|o| o.norm == METHOD && o.style == "camelCase")
            .map(|o| {
                let fix = o.fix.first().map(|(_, replaces)| {
                    let at: Vec<String> = (replaces.iter())
                        .map(|r| format!("{}@{}", r.text, index.line(r.span.start)))
                        .collect();
                    at.join(" ")
                });
                format!("{}: {}", o.line, fix.unwrap_or_default())
            })
            .collect()
    }

    #[test]
    fn only_private_instance_methods_get_a_rename() {
        let src = "class A\n  def run\n    doIt\n    self.doIt\n  end\n\n  def publicName; end\n\n  \
                   private\n\n  def doIt; end\n  def self.klassName; end\n  \
                   def symbolName; end\n  alias_method :other, :symbolName\nend\n";
        assert_eq!(
            renames(src),
            ["7: ", "11: do_it@3 do_it@4 do_it@11", "12: ", "13: ",]
        );
    }

    #[test]
    fn private_def_and_a_taken_name_decide_the_rename() {
        let src =
            "class A\n  private def getIt; end\n  private def setIt; end\n  def set_it; end\nend\n";
        assert_eq!(renames(src), ["2: get_it@2", "3: "]);
    }

    #[test]
    fn a_rename_keeps_the_prefix_and_needs_calls_by_the_same_name() {
        let src = "class A\n  def a\n    _getIt\n    isOn\n  end\n\n  private\n\n  \
                   def _getIt; end\n  def isOn?; end\nend\n";
        assert_eq!(renames(src), ["9: get_it@3 get_it@9", "10: "]);
    }
}
