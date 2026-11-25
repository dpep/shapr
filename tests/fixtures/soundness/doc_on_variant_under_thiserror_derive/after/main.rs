use thiserror::Error;

#[derive(Debug, Error)]
enum Failure {
    /// The file isn't there.
    #[error("not found")]
    Missing,
}
