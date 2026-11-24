//! End to end: default arguments, for what fixtures can't say (`reason`).

mod common;
use common::*;

const FETCHER: &str = "class Fetcher\n  def fetch(key, retries: 3)\n    [key, retries]\n  end\n\n  def go(x)\n    CALL\n  end\nend\n";

fn reason(call_before: &str, call_after: &str) -> String {
    let report = ruby(
        &FETCHER.replace("CALL", call_before),
        &FETCHER.replace("CALL", call_after),
    );
    lead(&hunks(&report)[0])["reason"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn the_reason_names_the_argument_and_the_method() {
    assert_eq!(
        reason("fetch(x, retries: 3)", "fetch(x)"),
        "drops `retries: 3`, `fetch`'s default"
    );
    assert_eq!(
        reason("x.fetch(x)", "x.fetch(x, retries: 3)"),
        "adds `retries: 3`, but the call has a receiver, so which `fetch` it reaches is unknown"
    );
}
