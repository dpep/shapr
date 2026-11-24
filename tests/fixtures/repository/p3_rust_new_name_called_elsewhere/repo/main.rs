mod inherent;

pub struct S;

pub trait Greet {
    fn hello(&self) -> u32;
}

impl Greet for S {
    fn hello(&self) -> u32 {
        1
    }
}

fn main() {
    let s = S;
    println!("{}", s.hello());
    println!("{}", inherent::describe(&s));
}
