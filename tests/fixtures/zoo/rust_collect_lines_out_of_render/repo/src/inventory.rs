use std::collections::BTreeMap;

pub struct Item {
    pub name: String,
    pub count: u32,
}

pub struct Inventory {
    items: BTreeMap<String, Item>,
}

impl Inventory {
    pub fn new() -> Self {
        Inventory {
            items: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, name: &str, qty: u32) {
        let name = name.to_string();
        let item = Item { name: name.clone(), count: qty };
        self.items.insert(name, item);
    }

    pub fn names(&self) -> Vec<&str> {
        self.items.keys().map(|k| k.as_str()).collect()
    }

    pub fn total(&self) -> u32 {
        let acc = self.sum_counts();
        acc
    }

    fn sum_counts(&self) -> u32 {
        let mut acc = 0;
        for item in self.items.values() {
            acc += item.count;
        }
        acc
    }

    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.values()
    }

    fn bump(&self, n: u32) -> u32 {
        n + 1
    }

    pub fn bumped_total(&self) -> u32 {
        self.bump((self.total()))
    }
}
