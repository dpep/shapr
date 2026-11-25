pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self) {
        self.items.iter().for_each(|i| println!("{}", i));
        self.print_doubled();
        println!("done");
    }

    fn print_doubled(&self) {
        println!("doubled");
        self.items.iter().for_each(|i| eprintln!("{}", i * 2));
    }
}
