pub struct Foo(u32);

pub trait Tr {
    fn value(&self) -> u32 {
        2
    }
}

impl Tr for Foo {}

impl Foo {
    pub fn keep(&self) -> u32 {
        self.0 * 7
    }
}

impl Foo {
    #![cfg(feature = "extra")]

    pub fn other(&self) -> u32 {
        self.0 * 3
    }

    pub fn value(&self) -> u32 {
        self.0 + 1
    }
}
