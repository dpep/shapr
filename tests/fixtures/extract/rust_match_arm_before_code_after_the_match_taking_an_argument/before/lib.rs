pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self, label: String, mode: u8) {
        match mode {
            0 => {
                println!("{}", label.len());
                self.items.iter().for_each(|i| eprintln!("{} {}", label.trim(), i));
            }
            _ => {
                println!("quiet");
            }
        }
        println!("done");
    }
}
