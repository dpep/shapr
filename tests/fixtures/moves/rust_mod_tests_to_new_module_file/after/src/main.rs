mod triple;
use triple::triple;

fn double(x: u32) -> u32 {
    x * 2
}

fn main() {
    println!("{}", double(triple(1)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles() {
        assert_eq!(double(2), 4);
        assert_eq!(double(0), 0);
    }
}
