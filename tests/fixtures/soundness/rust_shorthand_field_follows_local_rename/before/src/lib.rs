#[derive(Default)]
struct S {
    a: i32,
    c: i32,
}

fn f() -> S {
    let a = 1;
    S { a, ..Default::default() }
}
