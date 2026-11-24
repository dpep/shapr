//! shapr classifies the shape of a code change: per hunk, what kind of change
//! it is, and for the whole change whether it alters structure, behavior, or
//! both.

pub mod change;
pub mod claims;
pub mod classify;
mod cli;
pub mod diff;
pub mod frontend;
pub mod git;
pub mod github;
pub mod log;
pub mod norms;
pub mod report;
pub mod tree;

pub use cli::run;
