/// Mixes `a` into a number.
///
/// ```
/// assert_eq!(demo::helper(1), 4);
/// ```
pub fn run(a: u32) -> u32 {
    helper(a)
}

pub fn helper(a: u32) -> u32 {
    let t = a * 2 + 1;
    let u = t + a * 3;
    u - t
}
