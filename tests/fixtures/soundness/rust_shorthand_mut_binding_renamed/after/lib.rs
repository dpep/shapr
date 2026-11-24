struct S {
    a: i32,
    c: i32,
}

fn f(s: S) -> i32 {
    let S { mut c, .. } = s;
    c += 1;
    c
}
