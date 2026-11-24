#[derive(Debug, Default)]
struct S {
    a: i32,
    c: i32,
}

fn main() {
    let a = 1;
    let v = vec![S { a, ..Default::default() }];
    println!("{:?}", v);
}
