use crate::inventory::Inventory;

pub fn render(inv: &Inventory) -> String {
    let mut lines = Vec::new();
    for item in inv.items() {
        lines.push(format!("{}: {}", item.name, item.qty));
    }
    lines.join("\n")
}



pub fn summary(inv: &Inventory) -> String {
    format!("{} items", inv.names().len())
}

pub fn pad(s: &str, width: usize) -> String {
    format!("{:width$}", s, width = width)
}
