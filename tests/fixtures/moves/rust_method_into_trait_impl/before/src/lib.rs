pub struct A(u32);
pub trait Show {
    fn show(&self) -> u32;
}

impl A {
    fn show(&self) -> u32 {
        self.0 * 10 + 1
    }
}

impl Show for A {
    fn show(&self) -> u32 {
        self.0 * 10 + 1
    }
}
