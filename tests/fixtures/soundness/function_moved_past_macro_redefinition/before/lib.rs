macro_rules! a { () => { 1 } }
fn f() -> i32 { a!() }
macro_rules! a { () => { 2 } }
fn g() {}
