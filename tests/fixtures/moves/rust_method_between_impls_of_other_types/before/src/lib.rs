pub struct A(u32);
pub struct B(u32);

impl A {
    fn scaled(&self) -> u32 {
        self.0 * 10 + 1
    }
}

impl B {
    fn zero(&self) -> u32 {
        self.0 - self.0
    }
}
