use zoo_rust::inventory::Inventory;
use zoo_rust::output::render;

fn main() {
    let mut inv = Inventory::new();
    inv.add("apple", 3);
    inv.add("pear", 2);
    println!("{}", render(&inv));
    println!("{}", zoo_rust::docs::ShowCmd::help());
    println!("{} {}", zoo_rust::shape::dump(&zoo_rust::shape::origin()), zoo_rust::shape::kind());
    println!("{}", zoo_rust::shape::mark_name());
    println!("{} {}", zoo_rust::output::size_word(3), zoo_rust::output::size_word(10));
}
