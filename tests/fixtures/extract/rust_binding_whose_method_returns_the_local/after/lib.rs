pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn summary(&self) -> String {
        let total = self.compute();
        format!("{total} items")
    }

    fn compute(&self) -> u32 {
        let total = self.items.iter().sum::<u32>() * 3 + self.items.len() as u32;
        total
    }
}
