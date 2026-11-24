use std::collections::BTreeMap;

pub struct Item {
    pub name: String,
    pub qty: u32,
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
        let item = Item { name: name.clone(), qty };
        self.items.insert(name, item);
    }

    pub fn total(&self) -> u32 {
        let mut acc = 0;
        for item in self.items.values() {
            acc += item.qty;
        }
        acc
    }

    pub fn names(&self) -> Vec<&str> {
        self.items.keys().map(|k| k.as_str()).collect()
    }

    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.items.values()
    }

    fn helper(&self, x: u32) -> u32 {
        x + 1
    }

    pub fn bumped_total(&self) -> u32 {
        self.helper((self.total()))
    }
}
