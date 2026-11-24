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
