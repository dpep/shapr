macro_rules! name_of {
    ($x:ident) => {
        stringify!($x)
    };
}

fn main() {
    let c = 1;
    println!("{} {}", name_of!(c), c);
}
