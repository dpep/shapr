fn parse(input: &str) -> Option<u8> {
    let number: Option<u8> = try { input.parse::<u8>().ok()? + 1 };
    number
}
