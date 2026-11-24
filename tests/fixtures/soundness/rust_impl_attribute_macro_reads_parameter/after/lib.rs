struct A;
#[pymethods]
impl A {
    fn get(&self, b: i32) -> i32 {
        b + 1
    }
}
