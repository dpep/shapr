macro_rules! with_help {
    ($(#[doc = $d:literal])* $v:vis struct $n:ident;) => {
        $v struct $n;
        impl $n { const HELP: &str = concat!($($d),*); }
    };
}
with_help! {
    /// Shows the inventory.
    pub struct ShowCmd;
}
