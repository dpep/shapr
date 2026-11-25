pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self, label: String, verbose: bool) {
        if verbose {
            self.each(label);
        }
        println!("done");
    }

    fn each(&self, label: String) {
        println!("{}", label.len());
        self.items.iter().for_each(|i| eprintln!("{} {}", label.trim(), i));
    }
}
