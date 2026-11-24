struct Mark;

pub fn mark_name() -> &'static str {
    std::any::type_name::<Mark>()
}
