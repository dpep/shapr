pub fn open(path: &str) -> Result<(), String> {
    let fresh = path.len() == 0;
    let _open = take(path, fresh)?;
    Ok(())
}
