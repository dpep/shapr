pub fn first(xs: &[u32]) -> Option<u32> {
    if xs.is_empty() {
        return None;
    }
    Some(xs[0])
}
