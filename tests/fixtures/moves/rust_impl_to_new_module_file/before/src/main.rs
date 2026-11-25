pub struct Parsed {
    count: usize,
}

impl Parsed {
    fn new() -> Self {
        Parsed { count: 0 }
    }

    fn bump(&mut self) -> usize {
        self.count += 1;
        self.count
    }

    fn total(&self) -> usize {
        self.count * 2 + 1
    }
}

fn main() {
    let mut p = Parsed::new();
    p.bump();
    println!("{}", p.total());
}
