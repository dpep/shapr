pub fn bucket(n: i64) -> &'static str {
    match n {
        1..=10 => "small",
        5..=20 => "mid",
        _ => "other",
    }
}
