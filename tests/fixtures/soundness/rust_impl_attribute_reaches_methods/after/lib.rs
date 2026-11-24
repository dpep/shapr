struct A;
#[pymethods]
impl A {
    fn bar(&self) -> i32 { 1 }
}
