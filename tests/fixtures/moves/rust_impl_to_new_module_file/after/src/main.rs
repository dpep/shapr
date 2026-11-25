mod parsed;
use parsed::Parsed;

fn main() {
    let mut p = Parsed::new();
    p.bump();
    println!("{}", p.total());
}
