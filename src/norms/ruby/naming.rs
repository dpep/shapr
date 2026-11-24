//! Naming/MethodName: method names in `snake_case` or `camelCase`. Only a
//! name of two or more words chooses: `total` reads the same in both.

use super::Walker;
use crate::norms::Replace;
use ruby_prism::Node;
use std::ops::Range;

pub const METHOD: &str = "Naming/MethodName";
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
