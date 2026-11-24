pub struct Inventory {
    items: Vec<u32>,
}

impl Inventory {
    pub fn add(&mut self, n: u32) {
        self.items.push(n);
    }

    pub fn total(&self) -> u32 {
        let mut acc = 0;
        for item in &self.items {
            acc += item;
        }
        acc
    }

    pub fn count(&self) -> usize {
        self.items.iter().filter(|n| **n > 0).count()
    }

    pub fn first(&self) -> Option<&u32> {
        self.items.first()
    }
}
