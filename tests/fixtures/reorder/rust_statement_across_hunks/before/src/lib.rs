pub fn f(repo: &str, store: &[u8]) -> usize {
    // Count the parts, which is cheap
    // and worth naming.
    let parts =
        count(repo, 0);
    let a = 1;
    let b = 2;
    let c = 3;
    let d = 4;
    let e = 5;
    let g = 6;
    let h = 7;
    let last = store
        .len();
    let stdlib = width(repo, last);
    parts + stdlib + a + b + c + d + e + g + h
}
