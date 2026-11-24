// Mimics derive/attribute macros (clap, schemars) that read doc comments.
macro_rules! with_help {
    ($(#[doc = $d:literal])* pub struct $n:ident;) => {
        $(#[doc = $d])*
        pub struct $n;
        impl $n {
            pub fn help() -> String {
                let mut s = String::new();
                $(s.push_str($d);)*
                s
            }
        }
    };
}

with_help! {
    /// Prints every item.
    pub struct ShowCmd;
}
