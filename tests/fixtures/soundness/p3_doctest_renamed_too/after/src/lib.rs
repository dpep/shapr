/// Splits text into words.
///
/// ```
/// assert_eq!(demo::words("a b"), vec!["a", "b"]);
/// ```
pub fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

pub fn count(text: &str) -> usize {
    words(text).len()
}
