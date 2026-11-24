#[derive(Debug, Default)]
struct S {
    a: i32,
    c: i32,
}

fn main() {
    let c = 1;
    let v = vec![S { c, ..Default::default() }];
    println!("{:?}", v);
}
