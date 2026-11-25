fn parse(input: &str) -> Result<u8, ()> {
    let value = tri!(input.parse::<u8>().map_err(|_| ()));
    Ok(value + 1)
}
