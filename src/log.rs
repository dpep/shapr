//! `-v` telemetry, always to stderr.

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// `eprintln!` under `-v`.
#[macro_export]
macro_rules! verbose {
    ($($arg:tt)*) => {
        if $crate::log::verbose() {
            // Paths and PR titles are untrusted: escape control characters.
            eprintln!("shapr: {}", $crate::report::escape(&format!($($arg)*)));
        }
    };
}
