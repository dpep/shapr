pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn render(&self) -> usize {
        self.print_header();
        header
    }

    fn print_header(&self) {
        if self.items.is_empty() {
            return;
        }
        let header = self.items.len() * 2 + self.items.iter().count();
        println!("{header}");
    }
}
