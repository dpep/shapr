use thiserror::Error;

#[derive(Debug, Error)]
enum Failure {
    #[error("not found")]
    Missing,
}
