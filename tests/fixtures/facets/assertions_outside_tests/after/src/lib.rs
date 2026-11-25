pub fn evict(items: &mut Vec<u32>, size: usize) {
    while items.len() > size {
        items.remove(0);
    }
}
