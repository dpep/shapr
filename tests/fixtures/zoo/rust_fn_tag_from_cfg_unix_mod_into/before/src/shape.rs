#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

pub fn origin() -> Point {
    Point { x: 0, y: 0 }
}

pub fn dump(p: &Point) -> String {
    format!("{:?}", p)
}

pub fn kind() -> &'static str {
    std::any::type_name::<Point>()
}

struct Tag;

pub fn mark_name() -> &'static str {
    std::any::type_name::<Tag>()
}

pub struct A;
pub struct B;

impl A {
    pub fn pad_a(&self) -> u32 {
        7
    }
}

impl B {
    pub fn id(&self) -> u32 {
        let base = 10;
        base + 1
    }

    pub fn pad_b(&self) -> u32 {
        8
    }
}

#[cfg(unix)]
mod unix_side {
    pub fn tag() -> &'static str {
        "unix"
    }
}

#[cfg(windows)]
mod win_side {
}
