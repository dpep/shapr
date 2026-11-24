struct A;
#[pymethods]
impl A {
    fn foo(&self) -> i32 { 1 }
}
