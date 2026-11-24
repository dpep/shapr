use zoo::report::{render, brief};

#[test]
fn t() {
    assert_eq!(render(), 1);
    assert_eq!(brief(), 2);
}
