pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn summary(&self, label: String) -> String {
        let n = label.trim().len() + self.items.iter().count();
        format!("{n} items")
    }
}
