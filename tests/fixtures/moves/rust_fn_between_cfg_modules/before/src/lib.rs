#[cfg(unix)]
mod imp {
    pub fn path_sep() -> char {
        let sep = '/';
        sep
    }
}

#[cfg(windows)]
mod imp {
    pub fn drive() -> char {
        'C'
    }
}
