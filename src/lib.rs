//! shapr classifies the shape of a code change: per hunk, what kind of change
//! it is, and for the whole change whether it alters structure, behavior, or
//! both.

mod change;
mod claims;
mod classify;
mod cli;
mod diff;
mod frontend;
mod git;
mod github;
mod log;
mod norms;
mod outside;
mod report;
mod tree;
mod usage;

pub use cli::run;
