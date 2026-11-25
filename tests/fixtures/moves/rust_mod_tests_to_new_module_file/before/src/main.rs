fn double(x: u32) -> u32 {
    x * 2
}

fn triple(x: u32) -> u32 {
    x * 3
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

    #[test]
    fn triples() {
        assert_eq!(triple(2), 6);
        assert_eq!(triple(0), 0);
    }
}
