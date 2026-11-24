/// Splits text into words.
///
/// ```
/// assert_eq!(demo::split_words("a b"), vec!["a", "b"]);
/// ```
pub fn split_words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

pub fn count(text: &str) -> usize {
    split_words(text).len()
}
