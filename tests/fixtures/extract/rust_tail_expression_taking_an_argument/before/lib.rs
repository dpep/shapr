pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn summary(&self, label: String) -> usize {
        label.trim().len() + self.items.iter().count()
    }
}
