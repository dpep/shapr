pub(super) fn total(xs: &[u32]) -> u32 {
    let mut sum = 0;
    for x in xs {
        sum += x;
    }
    sum
}
