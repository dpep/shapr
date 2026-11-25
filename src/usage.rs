//! The usage log: one NDJSON line per run (of `classify`, `norms`, or a
//! command answered before them: `--status`, `--usage`, ...), with
//! names and counts only (never paths, repositories, revisions, code,
//! messages or URLs), and its summary for `--usage`. Logging never fails a
//! run: an IO error is only logged under `-v`.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Past this size the log keeps its newest half.
const CAP: u64 = 5_000_000;

/// What the run so far has said about itself; written when it ends.
#[derive(Default)]
struct Run {
    command: Option<&'static str>,
    input: Option<crate::report::Input>,
    paths: Option<bool>,
    format: Option<&'static str>,
    exit: u8,
    counts: Map<String, Value>,
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);

fn with(f: impl FnOnce(&mut Run)) {
    if let Ok(mut run) = RUN.lock() {
        f(run.get_or_insert_with(Run::default));
    }
}

/// The run is a COMMAND run, and gets a line.
pub fn command(name: &'static str) {
    with(|r| r.command = Some(name));
}

pub fn input(input: crate::report::Input, paths: bool) {
    with(|r| {
        r.input = Some(input);
        r.paths = Some(paths);
    });
}

pub fn format(format: crate::report::Format) {
    let name = match format {
        crate::report::Format::Text => "text",
        crate::report::Format::Json => "json",
        crate::report::Format::Ndjson => "ndjson",
    };
    with(|r| r.format = Some(name));
}

pub fn exit(code: u8) {
    with(|r| r.exit = code);
}

/// KEYS of RECORD (a serialized summary), as the run's counts.
pub fn counts(record: &impl Serialize, keys: &[&str]) {
    let Ok(Value::Object(record)) = serde_json::to_value(record) else {
        return;
    };
    with(|r| {
        for key in keys {
            if let Some(value) = record.get(*key) {
                r.counts.insert(key.to_string(), value.clone());
            }
        }
    });
}

/// VALUE as the run's count under KEY.
pub fn count(key: &str, value: &impl Serialize) {
    let Ok(value) = serde_json::to_value(value) else {
        return;
    };
    with(|r| {
        r.counts.insert(key.to_string(), value);
    });
}

/// Where the log is, and whether this run writes to it. `SHAPR_USAGE=0`
/// (`false`, `off`, `no`) turns it off, `1` (`true`, `on`, `yes`) on, and
/// an absolute path moves it; unset, it is on except in a debug build or
/// in CI (`in_ci`).
pub struct Location {
    pub path: Option<PathBuf>,
    pub enabled: bool,
}

pub fn location() -> Location {
    let setting = std::env::var("SHAPR_USAGE").ok().filter(|v| !v.is_empty());
    let default = default_path();
    let word = setting.as_deref().map(str::to_ascii_lowercase);
    match (word.as_deref(), setting.as_deref()) {
        (Some("0" | "false" | "off" | "no"), _) => Location {
            path: default,
            enabled: false,
        },
        (Some("1" | "true" | "on" | "yes"), _) => Location {
            enabled: default.is_some(),
            path: default,
        },
        // A relative path would make a log per working directory.
        (_, Some(path)) if Path::new(path).is_absolute() => Location {
            path: Some(PathBuf::from(path)),
            enabled: true,
        },
        (_, Some(path)) => {
            crate::verbose!("SHAPR_USAGE={path} isn't absolute: the usage log is off");
            Location {
                path: default,
                enabled: false,
            }
        }
        (_, None) => {
            let ci = in_ci(|name| std::env::var(name).ok());
            Location {
                enabled: default.is_some() && !cfg!(debug_assertions) && !ci,
                path: default,
            }
        }
    }
}

/// Running in CI, as CI services say: `CI` set to a true word (`true`, `1`,
/// `yes`, `on`, any case), or `GITHUB_ACTIONS=true`. VAR reads a variable.
fn in_ci(var: impl Fn(&str) -> Option<String>) -> bool {
    let on = |name: &str| {
        var(name)
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on"))
    };
    on("CI") || on("GITHUB_ACTIONS")
}

/// `$XDG_STATE_HOME/shapr/usage.jsonl`, else `~/.local/state/shapr/usage.jsonl`.
fn default_path() -> Option<PathBuf> {
    let absolute = |var: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let state =
        absolute("XDG_STATE_HOME").or_else(|| Some(absolute("HOME")?.join(".local/state")))?;
    Some(state.join("shapr").join("usage.jsonl"))
}

/// Appends the run's line, when it named its command and the log is on. FLAGS are the names of the flags given.
pub fn finish(started: Instant, flags: Vec<String>) {
    let Some(run) = RUN.lock().ok().and_then(|mut r| r.take()) else {
        return;
    };
    let Some(command) = run.command else {
        return;
    };
    let Location {
        path: Some(path),
        enabled: true,
    } = location()
    else {
        return;
    };
    let line = serde_json::json!({
        "ts": timestamp(SystemTime::now()),
        "version": env!("CARGO_PKG_VERSION"),
        "command": command,
        "input": run.input,
        "paths": run.paths,
        "flags": flags,
        "format": run.format,
        "exit": run.exit,
        "duration_ms": started.elapsed().as_millis() as u64,
        "counts": run.counts,
    });
    if let Err(error) = append(&path, &line.to_string()) {
        crate::verbose!("usage log {}: {error}", path.display());
    }
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // Never through a symlink left at the path, checked before and after
    // opening: the file written is the one the path names.
    let symlink = || std::io::Error::other("the log path is a symlink");
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(symlink());
    }
    if !regular(path) {
        return Err(std::io::Error::other("the log path isn't a file"));
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let named = std::fs::symlink_metadata(path)?;
        let opened = file.metadata()?;
        if named.file_type().is_symlink()
            || (named.dev(), named.ino()) != (opened.dev(), opened.ino())
        {
            return Err(symlink());
        }
    }
    // One run at a time, so concurrent runs neither interleave nor trim twice.
    file.lock()?;
    writeln!(file, "{line}")?;
    if file.metadata()?.len() > CAP {
        // In place, under the lock: a rename would leave other runs writing
        // to the file it replaced.
        let all = std::fs::read(path)?;
        let kept = newest_half(&all).to_vec();
        file.set_len(0)?;
        file.write_all(&kept)?;
    }
    Ok(())
}

/// The newest half of LOG, from a line boundary.
fn newest_half(log: &[u8]) -> &[u8] {
    let middle = log.len() / 2;
    match log[middle..].iter().position(|&b| b == b'\n') {
        Some(at) => &log[middle + at + 1..],
        None => &[],
    }
}

/// UTC, to the second: `2026-11-24T08:15:00Z`.
fn timestamp(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// `--usage`: what the log holds, counted.
#[derive(Serialize, Default)]
pub struct Summary {
    record: &'static str,
    /// The log, with the home directory as `~`; `None` with no home.
    pub path: Option<String>,
    pub enabled: bool,
    pub bytes: u64,
    pub runs: usize,
    pub first: Option<String>,
    pub last: Option<String>,
    pub commands: BTreeMap<String, u64>,
    pub inputs: BTreeMap<String, u64>,
    pub flags: BTreeMap<String, u64>,
    pub kinds: BTreeMap<String, u64>,
    pub verdicts: BTreeMap<String, u64>,
}

/// Whether PATH is a regular file or nothing yet: opening anything else (a
/// FIFO) can block.
fn regular(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.is_file(),
        Err(_) => true,
    }
}

/// Empties the log.
pub fn reset() -> std::io::Result<()> {
    match location().path {
        Some(path) if !regular(&path) => Err(std::io::Error::other("the log path isn't a file")),
        Some(path) if path.exists() => std::fs::write(path, b""),
        _ => Ok(()),
    }
}

pub fn summary() -> Summary {
    let location = location();
    let mut summary = Summary {
        record: "usage",
        path: location.path.as_deref().map(crate::github::tilde),
        enabled: location.enabled,
        ..Summary::default()
    };
    let readable = location.path.filter(|p| p.exists() && regular(p));
    let Some(text) = readable.and_then(|p| std::fs::read_to_string(p).ok()) else {
        return summary;
    };
    summary.bytes = text.len() as u64;
    for line in text.lines() {
        // A line cut short by a crash, or written by another version, is skipped.
        let Ok(Value::Object(run)) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        summary.runs += 1;
        let text = |key: &str| run.get(key).and_then(Value::as_str).map(str::to_string);
        if let Some(ts) = text("ts") {
            if summary.first.as_ref().is_none_or(|f| ts < *f) {
                summary.first = Some(ts.clone());
            }
            if summary.last.as_ref().is_none_or(|l| ts > *l) {
                summary.last = Some(ts);
            }
        }
        let count = |map: &mut BTreeMap<String, u64>, key: Option<String>| {
            if let Some(key) = key {
                *map.entry(key).or_default() += 1;
            }
        };
        count(&mut summary.commands, text("command"));
        count(&mut summary.inputs, text("input"));
        let counts = run.get("counts");
        count(
            &mut summary.verdicts,
            counts
                .and_then(|c| c.get("verdict"))
                .and_then(Value::as_str)
                .map(str::to_string),
        );
        for flag in run
            .get("flags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            count(&mut summary.flags, flag.as_str().map(str::to_string));
        }
        let kinds = counts
            .and_then(|c| c.get("kinds"))
            .and_then(Value::as_object);
        for (kind, n) in kinds.into_iter().flatten() {
            *summary.kinds.entry(kind.clone()).or_default() += n.as_u64().unwrap_or(0);
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn ci_is_any_true_word_or_github_actions() {
        let ci = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            in_ci(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()))
        };
        for value in ["true", "TRUE", "1", "yes", "on"] {
            assert!(ci(&[("CI", value)]), "CI={value}");
        }
        assert!(ci(&[("GITHUB_ACTIONS", "true")]));
        for value in ["", "0", "false", "no"] {
            assert!(!ci(&[("CI", value)]), "CI={value}");
        }
        assert!(!ci(&[]));
    }

    #[test]
    fn the_newest_half_starts_at_a_line() {
        assert_eq!(newest_half(b"aaaa\nbb\ncc\n"), b"cc\n");
        assert_eq!(newest_half(b"one long line\n"), b"");
        assert_eq!(newest_half(b""), b"");
    }

    #[test]
    fn timestamps_are_utc_seconds() {
        let at = |secs| timestamp(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_795_508_100), "2026-11-24T08:15:00Z");
    }
}
