use std::collections::VecDeque;

use abnegate_exec::executor::OutputLimiter;

/// One stream of a command's output, held to a fixed size however much the
/// command writes.
///
/// The first bytes are kept up to the limit and the last bytes in a ring of
/// the same size, so what comes back is the start and the end with the
/// middle dropped: the error a build was run for is usually at the end.
#[derive(Debug)]
pub(crate) struct Capture {
    head: Vec<u8>,
    limiter: OutputLimiter,
    tail: VecDeque<u8>,
    limit: usize,
    dropped: usize,
}

impl Capture {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            head: Vec::new(),
            limiter: OutputLimiter::new(limit),
            tail: VecDeque::new(),
            limit,
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        let (_, kept, _) = self.limiter.check(bytes.len());
        self.head.extend_from_slice(&bytes[..kept]);
        let rest = &bytes[kept..];
        let skipped = rest.len().saturating_sub(self.limit);
        self.dropped += skipped;
        self.tail.extend(&rest[skipped..]);
        let overflow = self.tail.len().saturating_sub(self.limit);
        self.dropped += overflow;
        self.tail.drain(..overflow);
    }

    /// The kept output as text, with a marker where the middle was dropped.
    pub(crate) fn text(&self) -> String {
        let head = String::from_utf8_lossy(&self.head);
        if self.tail.is_empty() {
            return head.into_owned();
        }
        let (front, back) = self.tail.as_slices();
        let tail = String::from_utf8_lossy(&[front, back].concat()).into_owned();
        if self.dropped == 0 {
            return format!("{head}{tail}");
        }
        format!(
            "{head}\n[… {} bytes of output dropped …]\n{tail}",
            self.dropped
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_under_the_limit_is_kept_whole() {
        let mut capture = Capture::new(8);
        capture.push(b"abc");
        capture.push(b"def");
        assert_eq!(capture.text(), "abcdef");
    }

    #[test]
    fn output_between_one_and_two_limits_is_kept_whole() {
        let mut capture = Capture::new(4);
        capture.push(b"abcdef");
        assert_eq!(capture.text(), "abcdef");
    }

    #[test]
    fn a_flood_keeps_its_start_and_end_and_nothing_else() {
        let mut capture = Capture::new(4);
        capture.push(b"HEAD");
        for _ in 0..10_000 {
            capture.push(b"middle");
        }
        capture.push(b"TAIL");

        assert_eq!(capture.head.len() + capture.tail.len(), 8);
        let text = capture.text();
        assert!(text.starts_with("HEAD\n[… "), "{text}");
        assert!(text.ends_with(" dropped …]\nTAIL"), "{text}");
        assert!(text.contains(&format!("{} bytes", 10_000 * 6)), "{text}");
    }
}
