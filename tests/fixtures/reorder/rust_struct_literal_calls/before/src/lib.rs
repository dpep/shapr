pub struct S {
    pub a: u8,
    pub b: u8,
}

fn g() -> u8 {
    2
}

pub fn f() -> S {
    S {
        b: g(),
        a: 1,
    }
}
