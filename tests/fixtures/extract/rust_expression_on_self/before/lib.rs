pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn summary(&self) -> String {
        let n = self.items.iter().filter(|i| **i > 0).count();
        format!("{n} items")
    }
}
