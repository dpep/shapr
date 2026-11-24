macro_rules! name_of {
    ($x:ident) => {
        stringify!($x)
    };
}

fn main() {
    let a = 1;
    println!("{} {}", name_of!(a), a);
}
