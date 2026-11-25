mod imp {
    #![cfg(unix)]

    pub fn keep() -> u32 {
        7
    }
}

mod imp {
    #![cfg(windows)]

    pub fn other() -> u32 {
        3
    }

    pub fn value() -> u32 {
        1 + 100
    }
}
