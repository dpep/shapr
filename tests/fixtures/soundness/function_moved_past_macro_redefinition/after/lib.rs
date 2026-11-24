macro_rules! a { () => { 1 } }
macro_rules! a { () => { 2 } }
fn f() -> i32 { a!() }
fn g() {}
