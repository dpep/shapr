#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no such file")]
    Missing,
}
