#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("file not found")]
    Missing,
}
