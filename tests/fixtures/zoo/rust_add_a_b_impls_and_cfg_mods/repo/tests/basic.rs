use zoo_rust::inventory::Inventory;
use zoo_rust::report::{render, overview};
use zoo_rust::tokenizer::{count_words, tokenize};

#[test]
fn inventory_total() {
    let mut stock = Inventory::new();
    stock.add("a", 2);
    stock.add("b", 3);
    assert_eq!(stock.total(), 5);
    assert_eq!(stock.bumped_total(), 6);
}

#[test]
fn render_and_overview() {
    let mut stock = Inventory::new();
    stock.add("a", 2);
    assert_eq!(render(&stock), "a: 2");
    assert_eq!(overview(&stock), "1 items");
}

#[test]
fn words() {
    assert_eq!(count_words(&tokenize("a b 3")), 2);
}

#[test]
fn buckets() {
    assert_eq!(zoo_rust::tokenizer::bucket(3), "small");
    assert_eq!(zoo_rust::tokenizer::bucket(15), "mid");
}

#[test]
fn bucket_other() {
    assert_eq!(zoo_rust::tokenizer::bucket(99), "other");
}
