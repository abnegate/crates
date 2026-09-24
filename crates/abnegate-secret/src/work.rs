//! Walks over the text that count the bytes they read.
//!
//! The redaction and sanitization scanners walk their text only through these
//! helpers, so that a test can bound the bytes a scanner reads for each byte it
//! is given and prove it linear without a clock. A walk is any read that goes
//! further from the cursor than a fixed distance: a run, a search, a lookbehind,
//! or a pass over a candidate. The `source` tests refuse a scanner that walks
//! any other way, since the count cannot see such a walk.
//!
//! Under test every helper counts each index or byte it reads. Outside tests
//! the count is a no-op, so each helper is the iterator it wraps.

#[cfg(test)]
mod source;

#[cfg(test)]
use std::cell::Cell;
use std::iter;
use std::ops::Range;

/// Bytes a scanner may read for each byte of its input, counting every index
/// a walk visits: the byte that ends it, and each pass over a candidate or a
/// key, as well as the run itself.
#[cfg(test)]
const BYTES_READ_PER_BYTE: usize = 6;

/// Bytes that doubling the input may read beyond twice what the input read:
/// the scans at either end of an input do not repeat when it doubles.
#[cfg(test)]
const SLACK: usize = 64;

#[cfg(test)]
thread_local! {
    static SCANNED: Cell<usize> = const { Cell::new(0) };
}

/// The first value `found` returns, trying each index of `bytes` from `from`
/// forward.
pub(crate) fn find_map<T>(
    bytes: &[u8],
    from: usize,
    found: impl FnMut(usize) -> Option<T>,
) -> Option<T> {
    counted(from..bytes.len()).find_map(found)
}

/// The first index of `bytes`, from `from` forward, at which `found` holds.
pub(crate) fn find(
    bytes: &[u8],
    from: usize,
    mut found: impl FnMut(usize) -> bool,
) -> Option<usize> {
    counted(from..bytes.len()).find(|index| found(*index))
}

/// The last index of `bytes` at which `found` holds.
pub(crate) fn rfind(bytes: &[u8], mut found: impl FnMut(usize) -> bool) -> Option<usize> {
    counted(0..bytes.len()).rev().find(|index| found(*index))
}

/// Where the run of bytes that `within` admits, starting at `from`, ends.
pub(crate) fn run(bytes: &[u8], from: usize, mut within: impl FnMut(u8) -> bool) -> usize {
    find(bytes, from, |index| !within(bytes[index])).unwrap_or(bytes.len().max(from))
}

/// Where the run of bytes that `within` admits, ending at `range.end`, starts,
/// reading back no further than `range.start`.
pub(crate) fn run_back(
    bytes: &[u8],
    range: Range<usize>,
    mut within: impl FnMut(u8) -> bool,
) -> usize {
    let limit = range.start;
    counted(range)
        .rev()
        .find(|index| !within(bytes[*index]))
        .map_or(limit, |index| index + 1)
}

/// Whether `found` holds for some byte of `bytes`.
pub(crate) fn any(bytes: &[u8], mut found: impl FnMut(u8) -> bool) -> bool {
    counted(bytes.iter()).any(|byte| found(*byte))
}

/// Whether `within` holds for every byte of `bytes`.
pub(crate) fn all(bytes: &[u8], mut within: impl FnMut(u8) -> bool) -> bool {
    counted(bytes.iter()).all(|byte| within(*byte))
}

/// Hand each byte of `bytes` to `visit`, in order.
pub(crate) fn each(bytes: &[u8], mut visit: impl FnMut(u8)) {
    counted(bytes.iter()).for_each(|byte| visit(*byte));
}

/// Where each occurrence of `needle` in `text` starts, left to right and
/// without overlap, as [`str::match_indices`] finds them.
pub(crate) fn occurrences<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = usize> {
    let mut matches = text.match_indices(needle);
    let mut searched = 0;
    iter::from_fn(move || {
        let at = matches.next().map(|(at, _)| at);
        let end = at.map_or(text.len(), |at| at + needle.len());
        scanned(end - searched);
        searched = end;
        at
    })
}

// Every helper counts, through `scanned`, each index it tries and each byte it
// searches before it hands on what it found. A read it does not count is a
// read `assert_linear` cannot see, and a quadratic walk hidden there passes.
fn counted<I: DoubleEndedIterator>(items: I) -> impl DoubleEndedIterator<Item = I::Item> {
    items.inspect(|_| scanned(1))
}

/// Count `bytes` a walk read, toward the total that [`assert_linear`] bounds.
#[cfg(test)]
fn scanned(bytes: usize) {
    SCANNED.set(SCANNED.get() + bytes);
}

/// Count nothing: outside tests no count is kept, so [`counted`] is the
/// iterator it wraps and [`occurrences`] is [`str::match_indices`].
#[cfg(not(test))]
fn scanned(_: usize) {}

/// Build the input for `repetitions` and for twice as many, run `scan` over
/// each, and assert that the scanner is linear.
///
/// It is linear when it reads at most [`BYTES_READ_PER_BYTE`] bytes for each
/// byte of either input, and when doubling the input at most doubles the bytes
/// read, give or take [`SLACK`]. The second holds for a linear scan whatever
/// its constant and fails for any faster growth, so neither check leans on the
/// other. Both count only what goes through the helpers above, so every scanner
/// is first checked to walk through nothing else.
#[cfg(test)]
#[track_caller]
pub(crate) fn assert_linear(
    repetitions: usize,
    input: impl Fn(usize) -> String,
    scan: impl Fn(&str),
) {
    source::assert_every_walk_counted();

    let once = input(repetitions);
    let twice = input(2 * repetitions);
    let read = |text: &str| {
        SCANNED.set(0);
        scan(text);
        SCANNED.get()
    };
    let (read_once, read_twice) = (read(&once), read(&twice));

    let mut failures = Vec::new();
    for (text, read) in [(&once, read_once), (&twice, read_twice)] {
        if read > BYTES_READ_PER_BYTE * text.len() {
            failures.push(format!(
                "{read} bytes read for {} of input, over {BYTES_READ_PER_BYTE} a byte",
                text.len()
            ));
        }
    }
    if read_twice > 2 * read_once + SLACK {
        failures.push(format!(
            "doubling the input from {} to {} bytes took the bytes read from {read_once} to \
             {read_twice}, over twice as many and {SLACK} more",
            once.len(),
            twice.len()
        ));
    }
    assert!(
        failures.is_empty(),
        "the scan of input starting {:?} is not linear: {}",
        once.chars().take(24).collect::<String>(),
        failures.join("; ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counting<T>(walk: impl FnOnce() -> T) -> (T, usize) {
        SCANNED.set(0);
        let value = walk();
        (value, SCANNED.get())
    }

    #[test]
    fn each_helper_counts_every_index_or_byte_it_reads() {
        let bytes = b"ab  cd";
        let found = |index| (index == 3).then_some(9);
        assert_eq!(
            counting(|| find(bytes, 1, |index| bytes[index] == b'c')),
            (Some(4), 4)
        );
        assert_eq!(counting(|| find(bytes, 0, |_| false)), (None, 6));
        assert_eq!(counting(|| find_map(bytes, 2, found)), (Some(9), 2));
        assert_eq!(
            counting(|| rfind(bytes, |index| bytes[index] == b'b')),
            (Some(1), 5)
        );
        assert_eq!(counting(|| run(bytes, 0, |byte| byte != b' ')), (2, 3));
        assert_eq!(counting(|| run(bytes, 4, |byte| byte != b' ')), (6, 2));
        assert_eq!(counting(|| run(bytes, 7, |_| true)), (7, 0));
        assert_eq!(
            counting(|| run_back(bytes, 1..4, |byte| byte == b' ')),
            (2, 3)
        );
        assert_eq!(
            counting(|| run_back(bytes, 3..4, |byte| byte == b' ')),
            (3, 1)
        );
        assert_eq!(counting(|| any(bytes, |byte| byte == b'b')), (true, 2));
        assert_eq!(counting(|| all(bytes, |byte| byte != b' ')), (false, 3));
        assert_eq!(counting(|| each(bytes, |_| {})), ((), 6));
        let text = "ab  cd  ef";
        let found = || occurrences(text, "  ").collect::<Vec<_>>();
        assert_eq!(counting(found), (vec![2, 6], 10));
        assert_eq!(counting(|| occurrences(text, "  ").next()), (Some(2), 4));
        assert_eq!(counting(|| occurrences(text, "x").next()), (None, 10));
    }

    #[test]
    #[should_panic(expected = "bytes read for 1024 of input, over")]
    fn a_scan_over_its_budget_is_not_linear() {
        assert_linear(
            1024,
            |repetitions| "a".repeat(repetitions),
            |text| (0..=BYTES_READ_PER_BYTE).for_each(|_| each(text.as_bytes(), |_| {})),
        );
    }

    /// Reads twice its input at 16 KiB and three times at 32 KiB: within
    /// budget at both sizes, but growing faster than its input.
    #[test]
    #[should_panic(expected = "over twice as many")]
    fn a_scan_that_outgrows_its_input_within_budget_is_not_linear() {
        assert_linear(
            16 * 1024,
            |repetitions| "a".repeat(repetitions),
            |text| {
                let passes = text.len().ilog2() - 12;
                (0..passes).for_each(|_| each(text.as_bytes(), |_| {}));
            },
        );
    }

    #[test]
    #[should_panic(expected = "over twice as many")]
    fn a_quadratic_scan_is_not_linear() {
        assert_linear(
            256,
            |repetitions| "a".repeat(repetitions),
            |text| {
                let bytes = text.as_bytes();
                (0..bytes.len()).for_each(|from| {
                    run(bytes, from, |_| true);
                });
            },
        );
    }
}
