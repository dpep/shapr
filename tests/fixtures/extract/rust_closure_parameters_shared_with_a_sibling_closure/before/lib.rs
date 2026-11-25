pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self) {
        self.items.iter().for_each(|i| println!("{}", i));
        println!("doubled");
        self.items.iter().for_each(|i| eprintln!("{}", i * 2));
        println!("done");
    }
}
