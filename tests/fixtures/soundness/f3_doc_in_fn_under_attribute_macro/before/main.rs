#[my_macro::traced]
fn run() {
    /// Counts.
    let n = 1;
    drop(n);
}
