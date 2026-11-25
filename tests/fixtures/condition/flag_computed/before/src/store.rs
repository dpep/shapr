pub fn open(path: &str) -> Result<(), String> {
    let _open = take(path, false)?;
    Ok(())
}
