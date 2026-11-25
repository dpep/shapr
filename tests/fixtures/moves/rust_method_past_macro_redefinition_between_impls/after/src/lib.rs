pub struct Foo(u32);

macro_rules! bump {
    () => { 1 };
}

impl Foo {
    pub fn keep(&self) -> u32 {
        self.0 * 7
    }
}

macro_rules! bump {
    () => { 2 };
}

impl Foo {
    pub fn other(&self) -> u32 {
        self.0 * 3
    }

    pub fn value(&self) -> u32 {
        self.0 + bump!() + 100
    }
}
