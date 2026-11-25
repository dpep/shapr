fn parse(input: &str) -> Option<u8> {
    let value: Option<u8> = try { input.parse::<u8>().ok()? + 1 };
    value
}
