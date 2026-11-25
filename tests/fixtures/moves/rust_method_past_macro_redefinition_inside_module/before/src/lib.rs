pub struct Foo(u32);

mod imp {
    use super::Foo;

    macro_rules! bump {
        () => { 1 };
    }

    impl Foo {
        pub fn value(&self) -> u32 {
            self.0 + bump!() + 100
        }
    }

    macro_rules! bump {
        () => { 2 };
    }

    impl Foo {
        pub fn other(&self) -> u32 {
            self.0 * 3
        }
    }
}
