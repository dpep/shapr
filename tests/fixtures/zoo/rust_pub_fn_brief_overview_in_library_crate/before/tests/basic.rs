use zoo_rust::inventory::Inventory;
use zoo_rust::report::{render, brief};
use zoo_rust::tokenizer::{count_words, tokenize};

#[test]
fn inventory_total() {
    let mut inv = Inventory::new();
    inv.add("a", 2);
    inv.add("b", 3);
    assert_eq!(inv.total(), 5);
    assert_eq!(inv.bumped_total(), 6);
}

#[test]
fn render_and_brief() {
    let mut inv = Inventory::new();
    inv.add("a", 2);
    assert_eq!(render(&inv), "a: 2");
    assert_eq!(brief(&inv), "1 items");
}

#[test]
fn words() {
    assert_eq!(count_words(&tokenize("a b 3")), 2);
}
