mod uses_twice;
macro_rules! twice {
    ($x:expr) => {
        $x * 2
    };
}
