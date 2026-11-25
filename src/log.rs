//! `-v` telemetry, always to stderr.

use std::sync::atomic::{AtomicBool, Ordering};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// `report::note` under `-v`.
#[macro_export]
macro_rules! verbose {
    ($($arg:tt)*) => {
        if $crate::log::verbose() {
            $crate::report::note(&format!($($arg)*));
        }
    };
}

/// Time and calls per named phase, and counters, for the `-v` profile
/// line. Spans nest: a phase's time includes the phases run inside it.
static PROFILE: std::sync::Mutex<Vec<(&'static str, Entry)>> = std::sync::Mutex::new(Vec::new());

#[derive(Clone, Copy)]
enum Entry {
    Span(std::time::Duration, u64),
    Count(u64),
}

fn record(name: &'static str, add: Entry) {
    let Ok(mut profile) = PROFILE.lock() else {
        return;
    };
    match (profile.iter_mut().find(|(n, _)| *n == name), add) {
        (Some((_, Entry::Span(total, calls))), Entry::Span(time, n)) => {
            *total += time;
            *calls += n;
        }
        (Some((_, Entry::Count(total))), Entry::Count(n)) => *total += n,
        (Some(_), _) => {}
        (None, add) => profile.push((name, add)),
    }
}

/// Charges its lifetime to its phase when dropped.
pub struct Span(&'static str, std::time::Instant);

/// A span charging NAME until it drops.
pub fn span(name: &'static str) -> Span {
    Span(name, std::time::Instant::now())
}

impl Drop for Span {
    fn drop(&mut self) {
        record(self.0, Entry::Span(self.1.elapsed(), 1));
    }
}

/// Adds N to the counter NAME.
pub fn count(name: &'static str, n: u64) {
    record(name, Entry::Count(n));
}

/// `profile: total 812ms; diff 40ms; outside.parse 520ms ×3; parsed.bytes =1200; ...`,
/// in the order each first ended or counted.
pub fn profile(total: std::time::Duration) -> String {
    let profile = PROFILE.lock().map(|p| p.clone()).unwrap_or_default();
    let mut line = format!("profile: total {}ms", total.as_millis());
    for (name, entry) in profile {
        line.push_str(&match entry {
            Entry::Span(time, 1) => format!("; {name} {}ms", time.as_millis()),
            Entry::Span(time, calls) => format!("; {name} {}ms ×{calls}", time.as_millis()),
            Entry::Count(n) => format!("; {name} ={n}"),
        });
    }
    line
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_profile_names_spans_and_counters() {
        drop(super::span("test.span"));
        drop(super::span("test.span"));
        super::count("test.count", 3);
        super::count("test.count", 4);
        let line = super::profile(std::time::Duration::from_millis(5));
        assert!(line.starts_with("profile: total 5ms"), "{line}");
        assert!(line.contains("; test.span 0ms ×2"), "{line}");
        assert!(line.contains("; test.count =7"), "{line}");
    }
}
