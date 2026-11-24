// a
fn f() {}

fn p() {}
fn p() {}
fn p() {}
fn p() {}
fn p() {}
#[track_caller]
fn g() -> u32 {
    std::panic::Location::caller().line()
}
