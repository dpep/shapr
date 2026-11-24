/// A token of input.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Word(String),
    Number(i64),
}

/// Breaks text into whitespace-separated tokens.
///
/// ```
/// use zoo_rust::tokenizer::{tokenize, Token};
/// assert_eq!(tokenize("a 2"), vec![Token::Word("a".to_string()), Token::Number(2)]);
/// ```
pub fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for chunk in text.split_whitespace() {
        // try a number, else a word
        match chunk.parse::<i64>() {
            Ok(n) => out.push(Token::Number(n.abs())),
            Err(_) => out.push(Token::Word(chunk.to_string())),
        }
    }
    out
}

pub fn count_words(tokens: &[Token]) -> usize {
    tokens.iter().filter(|t| matches!(t, Token::Word(_))).count()
}

pub fn pad(s: &str, width: usize) -> String {
    format!("{:width$}", s, width = width)
}

pub fn bucket(n: i64) -> &'static str {
    match n {
        5..=20 => "mid",
        1..=10 => "small",
        _ => "other",
    }
}
