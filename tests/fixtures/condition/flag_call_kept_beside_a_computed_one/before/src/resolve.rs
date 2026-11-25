pub fn go(t: &Tree, o: &str, n: &str, singleton: bool) -> Option<u32> {
    let a = t.lookup(o, singleton, n)?.clone()?;
    let b = t.lookup(o, true, n)?.clone()?;
    Some(a + b)
}
