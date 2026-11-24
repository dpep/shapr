struct A;
#[pymethods]
impl A {
    fn get(&self, a: i32) -> i32 {
        a + 1
    }
}
