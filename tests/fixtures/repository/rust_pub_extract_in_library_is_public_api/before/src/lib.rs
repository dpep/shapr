pub struct Meter {
    base: u32,
    offset: u32,
}

impl Meter {
    pub fn read(&self, limit: u32) -> u32 {
        let total = self.base + self.offset;
        let clamped = total.min(limit);
        clamped * 3 + 1
    }
}
