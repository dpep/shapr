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

#[cfg(not(unix))]
pub fn sep() -> &'static str {
    "\\"
}

#[cfg(unix)]
pub fn sep() -> &'static str {
    "/"
}

#[allow(unreachable_patterns)]
pub fn size_word(v: i32) -> &'static str {
    use crate::limits::*;
    let _ = LIMIT;
    match v {
        other => {
            let _ = other;
            "small"
        }
        _ => "other",
    }
}
