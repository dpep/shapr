use zoo_rust::inventory::Inventory;
use zoo_rust::report::render;

fn main() {
    let mut inv = Inventory::new();
    inv.add("apple", 3);
    inv.add("pear", 2);
    println!("{}", render(&inv));
    println!("{}", zoo_rust::docs::ShowCmd::help());
}
