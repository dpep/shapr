pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self, xs: &[u32]) {
        for x in xs {
            let label = format!("{x}");
            println!("{}", label.len());
            self.items.iter().for_each(|i| eprintln!("{} {}", label.trim(), i));
        }
        println!("done");
    }
}
