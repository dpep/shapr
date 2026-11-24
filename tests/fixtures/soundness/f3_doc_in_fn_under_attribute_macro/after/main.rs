#[my_macro::traced]
fn run() {
    /// Tallies.
    let n = 1;
    drop(n);
}
