macro_rules! a { () => { 1 } }
fn f() -> i32 { a!() }
fn g() {}
