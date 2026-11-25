pub(super) fn triple(x: u32) -> u32 {
    x * 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples() {
        assert_eq!(triple(2), 6);
        assert_eq!(triple(0), 0);
    }
}
