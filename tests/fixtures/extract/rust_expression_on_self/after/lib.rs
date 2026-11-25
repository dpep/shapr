pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn summary(&self) -> String {
        let n = self.nonzero();
        format!("{n} items")
    }

    fn nonzero(&self) -> usize {
        self.items.iter().filter(|i| **i > 0).count()
    }
}
