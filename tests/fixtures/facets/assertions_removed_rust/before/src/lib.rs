pub fn evict(items: &mut Vec<u32>, size: usize) {
    while items.len() > size {
        items.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_oldest() {
        let mut items = vec![1, 2, 3];
        evict(&mut items, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], 2);
        assert!(!items.contains(&1));
    }
}
