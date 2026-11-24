pub struct S {
    pub a: u8,
    pub b: u8,
}

pub fn f(x: u8) -> S {
    S {
        a: 1,
        b: x,
    }
}
