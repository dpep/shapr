pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self, label: String) {
        let n = self.items.len();
        println!("{n}");
        self.each(label);
    }

    fn each(&self, label: String) {
        self.items.iter().for_each(|i| eprintln!("{} {}", label.trim(), i));
    }
}
