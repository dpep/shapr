pub struct Cart {
    items: Vec<u32>,
}

impl Cart {
    pub fn log(&self, label: String, mode: u8) {
        match mode {
            0 => {
                self.each(label);
            }
            _ => {
                println!("quiet");
            }
        }
        println!("done");
    }

    fn each(&self, label: String) {
        println!("{}", label.len());
        self.items.iter().for_each(|i| eprintln!("{} {}", label.trim(), i));
    }
}
