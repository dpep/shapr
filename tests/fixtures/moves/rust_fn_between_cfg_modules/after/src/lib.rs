#[cfg(unix)]
mod imp {}

#[cfg(windows)]
mod imp {
    pub fn drive() -> char {
        'C'
    }

    pub fn path_sep() -> char {
        let sep = '/';
        sep
    }
}
