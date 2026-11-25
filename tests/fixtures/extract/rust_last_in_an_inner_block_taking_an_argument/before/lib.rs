pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self, label: String, verbose: bool) {
        if verbose {
            println!("{}", label.len());
            self.items.iter().for_each(|i| eprintln!("{} {}", label.trim(), i));
        }
        println!("done");
    }
}
