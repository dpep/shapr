pub fn evict(items: &mut Vec<u32>, size: usize) {
    assert!(size > 0);
    debug_assert_ne!(items.len(), 0);
    while items.len() > size {
        items.remove(0);
    }
}
