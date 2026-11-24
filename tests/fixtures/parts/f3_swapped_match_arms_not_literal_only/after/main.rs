pub fn bucket(n: i64) -> &'static str {
    match n {
        5..=20 => "mid",
        1..=10 => "small",
        _ => "other",
    }
}
