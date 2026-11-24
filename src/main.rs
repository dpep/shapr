fn main() -> std::process::ExitCode {
    // Parsers and tree walks recurse once per nesting level; the depth limit
    // (`change::MAX_DEPTH`) bounds the core's, this the parsers' below it.
    let run = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(shapr::run)
        .expect("spawn the main thread");
    run.join().unwrap_or(std::process::ExitCode::from(2))
}
