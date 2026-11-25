pub fn ids() -> Vec<u32> {
    vec![2, 1]
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_sorted() {
        let mut v = super::ids();
        assert_eq!(v, [1, 2]);
    }
}
