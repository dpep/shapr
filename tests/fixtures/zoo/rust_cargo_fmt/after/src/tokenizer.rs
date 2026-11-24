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
    tokens
        .iter()
        .filter(|t| matches!(t, Token::Word(_)))
        .count()
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

pub fn mix(a: u32, b: u32) -> u32 {
    let t = a + 1;
    t * b
}

pub fn greet(name: &str) -> String {
    let person = name.trim();
    format!("hi {person}")
}

pub fn label(tag: &str) -> String {
    let person = "x";
    format!("{}-{}", tag, person)
}

pub fn show(n: i32) -> String {
    let m = n + 1;
    format!("{} {}", stringify!(m), m)
}

pub fn log_and_bump(counter: &mut u32, log: &mut Vec<String>) -> u32 {
    *counter += 1;
    log.push(format!("before {}", counter));
    log.push(format!("after {}", counter));
    *counter
}

pub fn big() -> u64 {
    let x = 1u64 << 20;
    x
}

pub fn lit() -> f64 {
    let x = 3.0;
    x / 2.0
}

pub fn shade() -> i32 {
    let x = 1;
    let y = x + 1;
    let x = 10;
    x + y
}

pub fn shade2(n: i32) -> i32 {
    let total = n;
    {
        let total = total * 2;
        let _ = total;
    }
    total
}
