fn go(a: u32) -> u32 {
    show(a);
    1
}

fn show(a: u32) {
    let t = a * 2 + 1;
    println!("{} {}", t, line!());
}
