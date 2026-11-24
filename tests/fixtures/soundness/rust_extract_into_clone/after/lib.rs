struct A;
impl A {
    fn go(&self, a: u32) -> u32 {
        self.clone(a)
    }

    fn clone(&self, a: u32) -> u32 {
        let t = a * 2 + 1;
        let u = t + a * 3;
        u - t
    }
}
