mod imp {
    #![cfg(unix)]

    pub fn keep() -> u32 {
        7
    }

    pub fn value() -> u32 {
        1 + 100
    }
}

mod imp {
    #![cfg(windows)]

    pub fn other() -> u32 {
        3
    }
}
