fn g<A, B>(x: i32) -> bool {
    x > 0
}

fn main() {
    let c = 1;
    assert!(g::<u8, u16>(c));
}
