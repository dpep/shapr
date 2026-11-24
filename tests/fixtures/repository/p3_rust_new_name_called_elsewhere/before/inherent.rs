use crate::S;

impl S {
    pub(crate) fn helper(&self) -> u32 {
        2
    }
}

pub fn describe(s: &S) -> u32 {
    s.helper()
}
