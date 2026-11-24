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
    for piece in text.split_whitespace() {
        // try a number, else a word
        match piece.parse::<i64>() {
            Ok(n) => out.push(Token::Number(n)),
            Err(_) => out.push(Token::Word(piece.to_string())),
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
