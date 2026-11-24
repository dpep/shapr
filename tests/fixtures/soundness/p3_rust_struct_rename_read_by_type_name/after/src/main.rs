struct Tag;

pub fn mark_name() -> &'static str {
    std::any::type_name::<Tag>()
}
