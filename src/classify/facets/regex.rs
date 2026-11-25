//! Whether one regular expression provably matches whatever another did:
//! the two parsed into a small tree and compared piece by piece, where
//! every step either keeps a piece or widens it (a quantifier's range
//! grows, a character class or an alternation gains members, an anchor is
//! dropped). Each step is monotone, so the whole is: a match of the old
//! pattern is a match of the new one.
//!
//! Constructs that break monotonicity (negative lookaround, atomic groups,
//! possessive quantifiers, backreferences, conditionals, inline flags) and
//! syntax this parser doesn't model make the answer unknown, never wrong.

/// A parsed pattern: its alternatives.
type Alt = Vec<Seq>;
type Seq = Vec<Piece>;

#[derive(Clone, Debug, PartialEq)]
struct Piece {
    atom: Atom,
    min: u32,
    /// `None`: unbounded.
    max: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
enum Atom {
    /// A character matching itself.
    Char(char),
    /// An escape matching a set or a character (`\d`, `\h`, `\.`): compared
    /// by its text.
    Escape(char),
    /// `.`
    Any,
    Class(Class),
    /// A group, by its opening (`(`, `(?:`, `(?<name>`, `(?=`, `(?i:`).
    Group(String, Alt),
    /// A zero-width anchor: `^`, `$`, `\A`, `\z`, `\Z`, `\b`, `\B`, `\G`.
    Anchor(String),
}

#[derive(Clone, Debug, PartialEq)]
struct Class {
    negated: bool,
    items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
enum Item {
    Char(char),
    Range(char, char),
    Escape(char),
}

/// How the new pattern relates to the old one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Change {
    /// It matches everything the old one did, through at least one step
    /// that accepts more where it is.
    Widened,
    /// Not shown wider: narrower, different, or beyond this parser.
    Changed,
}

/// Compares `old` with `new`, two pattern bodies of one dialect.
pub(super) fn compare(old: &str, new: &str) -> Change {
    let (Some(old), Some(new)) = (parse(old), parse(new)) else {
        return Change::Changed;
    };
    let mut strict = false;
    match alt_covers(&old, &new, &mut strict) && strict {
        true => Change::Widened,
        false => Change::Changed,
    }
}

fn parse(text: &str) -> Option<Alt> {
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    let alt = parse_alt(&chars, &mut at)?;
    (at == chars.len()).then_some(alt)
}

fn parse_alt(c: &[char], at: &mut usize) -> Option<Alt> {
    let mut alt = vec![parse_seq(c, at)?];
    while c.get(*at) == Some(&'|') {
        *at += 1;
        alt.push(parse_seq(c, at)?);
    }
    Some(alt)
}

fn parse_seq(c: &[char], at: &mut usize) -> Option<Seq> {
    let mut seq = Vec::new();
    while let Some(&ch) = c.get(*at) {
        if ch == '|' || ch == ')' {
            break;
        }
        let atom = parse_atom(c, at)?;
        let (min, max) = parse_quantifier(c, at)?;
        if matches!(atom, Atom::Anchor(_)) && (min, max) != (1, Some(1)) {
            return None;
        }
        seq.push(Piece { atom, min, max });
    }
    Some(seq)
}

fn parse_atom(c: &[char], at: &mut usize) -> Option<Atom> {
    let ch = c[*at];
    *at += 1;
    Some(match ch {
        '.' => Atom::Any,
        '^' | '$' => Atom::Anchor(ch.to_string()),
        '[' => Atom::Class(parse_class(c, at)?),
        '(' => {
            let open = group_opening(c, at)?;
            let alt = parse_alt(c, at)?;
            if c.get(*at) != Some(&')') {
                return None;
            }
            *at += 1;
            Atom::Group(open, alt)
        }
        '\\' => {
            let e = *c.get(*at)?;
            *at += 1;
            match e {
                'A' | 'z' | 'Z' | 'b' | 'B' | 'G' => Atom::Anchor(format!("\\{e}")),
                _ if multi_char_escape(e) => return None,
                _ => Atom::Escape(e),
            }
        }
        '*' | '+' | '?' | ')' => return None,
        _ => Atom::Char(ch),
    })
}

/// Escapes that read more than one character after the backslash, or name
/// an earlier group: not modelled.
fn multi_char_escape(e: char) -> bool {
    e.is_ascii_digit() || "kgpPxuoUcCMQEKXRN".contains(e)
}

/// A group's opening after `(`: `(` itself, `(?:`, a named group, a positive
/// lookaround, or flags scoped to the group (`(?i:`). `None` for the rest.
fn group_opening(c: &[char], at: &mut usize) -> Option<String> {
    if c.get(*at) != Some(&'?') {
        return Some("(".into());
    }
    let rest: String = c[*at..].iter().take(3).collect();
    for open in ["?:", "?=", "?<="] {
        if rest.starts_with(open) {
            *at += open.chars().count();
            return Some(format!("({open}"));
        }
    }
    // A named group: `(?<n>`, `(?P<n>`, `(?'n'`.
    let named = match (c.get(*at + 1), c.get(*at + 2)) {
        (Some('<'), Some(n)) if n.is_alphabetic() || *n == '_' => Some('>'),
        (Some('P'), Some('<')) => Some('>'),
        (Some('\''), _) => Some('\''),
        _ => None,
    };
    if let Some(close) = named {
        let end = (*at + 2..c.len()).find(|&i| c[i] == close)?;
        let open: String = c[*at..=end].iter().collect();
        *at = end + 1;
        return Some(format!("({open}"));
    }
    // Flags scoped to the group, never extended mode (`x`), which changes
    // how the rest reads.
    let flags: String = c[*at + 1..]
        .iter()
        .take_while(|f| f.is_ascii_alphabetic() || **f == '-')
        .collect();
    let colon = *at + 1 + flags.chars().count();
    if !flags.is_empty() && !flags.contains('x') && c.get(colon) == Some(&':') {
        *at = colon + 1;
        return Some(format!("(?{flags}:"));
    }
    None
}

fn parse_class(c: &[char], at: &mut usize) -> Option<Class> {
    let negated = c.get(*at) == Some(&'^');
    if negated {
        *at += 1;
    }
    let mut items = Vec::new();
    loop {
        let ch = *c.get(*at)?;
        *at += 1;
        let item = match ch {
            ']' if !items.is_empty() => return Some(Class { negated, items }),
            // A leading `]`, a nested class or `&&`: not modelled.
            ']' | '[' => return None,
            '&' if c.get(*at) == Some(&'&') => return None,
            '\\' => {
                let e = *c.get(*at)?;
                *at += 1;
                if multi_char_escape(e) || e == 'b' {
                    return None;
                }
                Item::Escape(e)
            }
            _ => Item::Char(ch),
        };
        // `a-z`; a `-` first or last is itself.
        let range_end = match (item.clone(), c.get(*at), c.get(*at + 1)) {
            (Item::Char(from), Some('-'), Some(&to)) if to != ']' && to != '\\' && to != '[' => {
                Some((from, to))
            }
            _ => None,
        };
        match range_end {
            Some((from, to)) if from <= to => {
                *at += 2;
                items.push(Item::Range(from, to));
            }
            Some(_) => return None,
            None => items.push(item),
        }
    }
}

/// The quantifier after an atom, as (min, max); (1, 1) when there is
/// none. `None` for a possessive one, which can make a wider piece match
/// less.
fn parse_quantifier(c: &[char], at: &mut usize) -> Option<(u32, Option<u32>)> {
    let (min, max) = match c.get(*at) {
        Some('*') => (0, None),
        Some('+') => (1, None),
        Some('?') => (0, Some(1)),
        Some('{') => match braces(c, *at) {
            Some((min, max, end)) => {
                *at = end;
                return suffix(c, at, min, max);
            }
            None => return Some((1, Some(1))),
        },
        _ => return Some((1, Some(1))),
    };
    *at += 1;
    suffix(c, at, min, max)
}

/// A lazy `?` reads past; a possessive `+` refuses.
fn suffix(c: &[char], at: &mut usize, min: u32, max: Option<u32>) -> Option<(u32, Option<u32>)> {
    match c.get(*at) {
        Some('?') => *at += 1,
        Some('+') => return None,
        _ => {}
    }
    Some((min, max))
}

/// `{n}`, `{n,}`, `{n,m}`, `{,m}` at `start`: (min, max, the index past
/// `}`). Anything else is a literal `{`.
fn braces(c: &[char], start: usize) -> Option<(u32, Option<u32>, usize)> {
    let end = (start + 1..c.len()).find(|&i| c[i] == '}')?;
    let inner: String = c[start + 1..end].iter().collect();
    let number = |s: &str| -> Option<u32> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (min, max) = match inner.split_once(',') {
        None => {
            let n = number(&inner)?;
            (n, Some(n))
        }
        Some(("", "")) => return None,
        Some((lo, "")) => (number(lo)?, None),
        Some(("", hi)) => (0, Some(number(hi)?)),
        Some((lo, hi)) => (number(lo)?, Some(number(hi)?)),
    };
    if max.is_some_and(|m| m < min) {
        return None;
    }
    Some((min, max, end + 1))
}

/// Every old alternative is covered by some new one.
fn alt_covers(old: &Alt, new: &Alt, strict: &mut bool) -> bool {
    let covered = old.iter().all(|o| {
        new.iter().any(|n| {
            let mut s = false;
            let ok = seq_covers(o, n, &mut s);
            *strict |= ok && s;
            ok
        })
    });
    // A new alternative no old one covers accepts more.
    if covered
        && new
            .iter()
            .any(|n| !old.iter().any(|o| seq_covers(n, o, &mut false)))
    {
        *strict = true;
    }
    covered
}

/// The new sequence is the old one with each piece kept or widened, and
/// anchors possibly dropped. Greedy, so it may miss a cover, never invent
/// one.
fn seq_covers(old: &Seq, new: &Seq, strict: &mut bool) -> bool {
    let (mut i, mut j) = (0, 0);
    while i < old.len() {
        if j < new.len() && piece_covers(&old[i], &new[j], strict) {
            i += 1;
            j += 1;
        } else if matches!(old[i].atom, Atom::Anchor(_)) {
            *strict = true;
            i += 1;
        } else {
            return false;
        }
    }
    j == new.len()
}

fn piece_covers(old: &Piece, new: &Piece, strict: &mut bool) -> bool {
    let wider_max = match (old.max, new.max) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some(o), Some(n)) => n >= o,
    };
    if new.min > old.min || !wider_max {
        return false;
    }
    let mut s = false;
    if !atom_covers(&old.atom, &new.atom, &mut s) {
        return false;
    }
    *strict |= s || new.min < old.min || new.max != old.max;
    true
}

fn atom_covers(old: &Atom, new: &Atom, strict: &mut bool) -> bool {
    match (old, new) {
        _ if old == new => true,
        (Atom::Char(c), Atom::Class(class)) if !class.negated => {
            let ok = class.items.iter().any(|i| item_covers(&Item::Char(*c), i));
            *strict |= ok && class.items != [Item::Char(*c)];
            ok
        }
        (Atom::Char(c), Atom::Any) if *c != '\n' => {
            *strict = true;
            true
        }
        (Atom::Class(o), Atom::Class(n)) if o.negated == n.negated => {
            // A negated class widens as it loses members.
            let (small, large) = match o.negated {
                false => (&o.items, &n.items),
                true => (&n.items, &o.items),
            };
            let ok = items_cover(small, large);
            *strict |= ok && !items_cover(large, small);
            ok
        }
        (Atom::Group(a, x), Atom::Group(b, y)) if a == b => alt_covers(x, y, strict),
        _ => false,
    }
}

fn items_cover(small: &[Item], large: &[Item]) -> bool {
    small
        .iter()
        .all(|s| large.iter().any(|l| item_covers(s, l)))
}

fn item_covers(small: &Item, large: &Item) -> bool {
    match (small, large) {
        _ if small == large => true,
        (Item::Char(c), Item::Range(a, b)) => a <= c && c <= b,
        (Item::Range(a, b), Item::Range(c, d)) => c <= a && b <= d,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widenings_are_proven() {
        for (old, new) in [
            (r"\h{2}", r"\h{1,2}"),
            (r"\A\h{2}(:\h{2}){5}\z", r"\A\h{1,2}(:\h{1,2}){5}\z"),
            ("a+", "a*"),
            ("ab", "ab?"),
            ("a{2,3}", "a{2,}"),
            ("[abc]", "[abcd]"),
            ("[a-f]", "[a-z]"),
            ("[^abc]", "[^ab]"),
            ("x", "[xy]"),
            ("x", "."),
            ("^foo$", "foo"),
            (r"\bfoo", "foo"),
            ("cat|dog", "cat|dog|bird"),
            ("(?:cat|dog)s", "(?:cat|dog|bird)s"),
            ("(?<id>[0-9]+)", "(?<id>[0-9]*)"),
            ("a{2}?", "a{1,2}?"),
        ] {
            assert_eq!(compare(old, new), Change::Widened, "{old} -> {new}");
        }
    }

    #[test]
    fn narrowings_rewrites_and_the_unmodelled_are_only_changed() {
        for (old, new) in [
            // Narrower.
            (r"\h{1,2}", r"\h{2}"),
            ("a*", "a+"),
            ("[abcd]", "[abc]"),
            ("[^ab]", "[^abc]"),
            ("foo", "^foo"),
            ("cat|dog|bird", "cat|dog"),
            // The same language, or no proof either way.
            ("a{1}", "a"),
            ("[abc]", "[cba]"),
            ("abc", "abd"),
            (r"\d", "[0-9]"),
            ("[a-c]", "[abc]"),
            // Monotonicity breaks inside these.
            ("(?!a{2})b", "(?!a{1,2})b"),
            ("(?>a{2})", "(?>a{1,2})"),
            ("a{2}+", "a{1,2}+"),
            (r"(a)\1{2}", r"(a)\1{1,2}"),
            ("(?x) a{2}", "(?x) a{1,2}"),
            ("(?i)a{2}", "(?i)a{1,2}"),
            (r"\p{L}{2}", r"\p{L}{1,2}"),
            ("[[:alpha:]]{2}", "[[:alpha:]]{1,2}"),
            // Malformed.
            ("a{3,2}", "a{1,3}"),
            ("(a", "(a|b"),
        ] {
            assert_eq!(compare(old, new), Change::Changed, "{old} -> {new}");
        }
    }

    #[test]
    fn a_brace_that_is_no_quantifier_is_itself() {
        assert_eq!(compare("a{x}", "a{x}?"), Change::Widened);
        assert_eq!(parse("{,}").map(|a| a[0].len()), Some(3));
    }
}
