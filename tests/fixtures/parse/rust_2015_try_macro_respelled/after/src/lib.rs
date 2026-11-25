fn parse(input: &str) -> Result<u8, ()> {
    let value = try!(input.parse::<u8>().map_err(|_| ()));
    Ok(value + 1)
}
