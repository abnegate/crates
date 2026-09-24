//! A backstop for the walk rule in `src/work/source.rs`, which refuses the
//! walks it can recognise in the scanners' source and no more: this times
//! [`redact`] and [`sanitize`] on the inputs most likely to make them rescan,
//! so a walk the rule misses still fails if it grows faster than its input.
//! Run it in a release build, as CI does:
//!
//! ```sh
//! cargo test -p abnegate-secret --release -- --ignored
//! ```

use std::hint::black_box;
use std::time::Duration;
use std::time::Instant;

use abnegate_secret::redact;
use abnegate_secret::sanitize;

/// Bytes in the shorter of the two inputs timed.
const LENGTH: usize = 16 * 1024;

/// How many times longer the longer input is.
const GROWTH: usize = 8;

/// How many times longer than the shorter input the longer may take: a linear
/// scan takes about [`GROWTH`] times as long and a quadratic one [`GROWTH`]
/// squared, so this sits halfway between, on a logarithmic scale.
const LIMIT: u32 = 32;

/// Runs of each input, of which only the fastest counts, so that a run the
/// machine interrupted does not.
const RUNS: usize = 5;

/// Inputs that make [`redact`] rescan if it can, each a prefix and a unit
/// repeated after it.
const REDACTED: &[(&str, &str)] = &[
    ("", "\"k\":1,"),
    ("http://x", ":1"),
    ("", "-----BEGIN "),
    ("", "-----BEGIN PRIVATE KEY "),
    ("", "password=a,"),
    ("password=", "a,b="),
    ("password=", "=a"),
    ("", "--password -"),
    ("", " \"!"),
    ("", "a"),
    ("", "ghp_"),
    ("", "eyJ"),
    ("", "password "),
    ("", "\""),
];

/// Inputs that make [`sanitize`] rescan if it can.
const SANITIZED: &[(&str, &str)] = &[
    ("", "\u{1b}]"),
    ("", "\u{1b}P"),
    ("", "\u{1b}X"),
    ("", "\u{1b}^"),
    ("", "\u{1b}_"),
    ("", "\u{1b}["),
    ("", "\u{9b}"),
    ("", "\u{200b}"),
];

#[test]
#[ignore = "times the scanners, so run it in a release build: cargo test -p abnegate-secret --release -- --ignored"]
fn each_scanner_takes_time_linear_in_its_input() {
    let failures = [
        failures("redact", redacting, REDACTED),
        failures("sanitize", sanitizing, SANITIZED),
    ]
    .concat();
    assert!(
        failures.is_empty(),
        "a scanner grew more than {LIMIT} times slower for {GROWTH} times the input:\n{}",
        failures.join("\n")
    );
}

fn redacting(text: &str) {
    black_box(redact(black_box(text)));
}

fn sanitizing(text: &str) {
    black_box(sanitize(black_box(text)));
}

/// How `scan`, the scanner `name`, grows too fast on each of `inputs`, a
/// prefix and a unit repeated after it: empty when it grows linearly on all.
fn failures(name: &str, scan: fn(&str), inputs: &[(&str, &str)]) -> Vec<String> {
    let mut failures = Vec::new();
    for (prefix, unit) in inputs {
        let input = |length: usize| format!("{prefix}{}", unit.repeat(length / unit.len()));
        let short = fastest(scan, &input(LENGTH));
        let long = fastest(scan, &input(GROWTH * LENGTH));
        let growth = long.as_secs_f64() / short.as_secs_f64();
        println!(
            "{name} {prefix:?} + {unit:?}: {short:?} for {LENGTH} bytes, {long:?} for {GROWTH} times as many, {growth:.1} times as long"
        );
        if long > short * LIMIT {
            failures.push(format!(
                "{name} of {prefix:?} and {unit:?} repeated took {long:?} for {} bytes, {growth:.1} times the {short:?} it took for {LENGTH}",
                GROWTH * LENGTH
            ));
        }
    }
    failures
}

/// The fastest of [`RUNS`] runs of `scan` over `text`.
fn fastest(scan: fn(&str), text: &str) -> Duration {
    (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            scan(text);
            start.elapsed()
        })
        .min()
        .unwrap_or_default()
}
