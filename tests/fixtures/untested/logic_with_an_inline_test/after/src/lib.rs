pub fn double(x: u32) -> u32 {
    x * 3
}





#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles() {
        assert_eq!(double(2), 6);
    }
}
