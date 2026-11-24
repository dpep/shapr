use zoo::report::{render, summary};

#[test]
fn t() {
    assert_eq!(render(), 1);
    assert_eq!(summary(), 2);
}
