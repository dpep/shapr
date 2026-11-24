#[derive(Default)]
struct S {
    a: i32,
    c: i32,
}

fn f() -> S {
    let c = 1;
    S { c, ..Default::default() }
}
