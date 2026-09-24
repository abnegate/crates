#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
const SCANS_PER_BYTE: usize = 4;

#[cfg(test)]
thread_local! {
    static SCANNED: Cell<usize> = const { Cell::new(0) };
}

/// Count the bytes a scan read ahead of or behind the cursor that started it,
/// so that tests can bound how often each byte of a text is read. Outside
/// tests it does nothing.
#[cfg(not(test))]
pub(crate) fn scanned(_: usize) {}

#[cfg(test)]
pub(crate) fn scanned(bytes: usize) {
    SCANNED.set(SCANNED.get() + bytes);
}

/// Run `scan` over `text` and assert that its scans read no more than
/// [`SCANS_PER_BYTE`] bytes for each byte of `text`.
#[cfg(test)]
#[track_caller]
pub(crate) fn assert_linear<'a, T>(text: &'a str, scan: impl FnOnce(&'a str) -> T) -> T {
    SCANNED.set(0);
    let value = scan(text);
    let scanned = SCANNED.get();
    assert!(
        scanned <= SCANS_PER_BYTE * text.len(),
        "scans of {} bytes starting {:?} read {scanned}, over {SCANS_PER_BYTE} a byte",
        text.len(),
        text.chars().take(24).collect::<String>()
    );
    value
}
