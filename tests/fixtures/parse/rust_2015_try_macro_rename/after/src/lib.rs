fn parse(input: &str) -> Result<u8, ()> {
    let number = try!(input.parse::<u8>().map_err(|_| ()));
    Ok(number + 1)
}
