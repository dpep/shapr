use crate::inventory::Inventory;

pub fn render(inv: &Inventory) -> String {
    let lines = collect_lines(inv);
    lines.join("\n")
}

fn collect_lines(inv: &Inventory) -> Vec<String> {
    let mut lines = Vec::new();
    for item in inv.items() {
        lines.push(format!("{}: {}", item.name, item.count));
    }
    lines
}



pub fn overview(inv: &Inventory) -> String {
    format!("{} items", inv.names().len())
}

#[cfg(unix)]
pub fn sep() -> &'static str {
    "/"
}

#[cfg(not(unix))]
pub fn sep() -> &'static str {
    "\\"
}
