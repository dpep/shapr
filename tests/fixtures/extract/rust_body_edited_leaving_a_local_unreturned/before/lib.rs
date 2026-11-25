pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn render(&self) -> usize {
        let header = self.items.len() * 2 + self.items.iter().count();
        println!("{header}");
        header
    }
}
