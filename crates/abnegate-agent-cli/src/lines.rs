//! Newline framing for a child process reading in arbitrary chunks.

use crate::error::Overlong;

const NEWLINE: u8 = b'\n';
const CARRIAGE_RETURN: &[u8] = b"\r";

/// How much of an overlong line is kept, which is far more than it takes to
/// tell what kind of event it was.
const PREFIX: usize = 1024;

/// Splits a byte stream into lines across chunk boundaries.
///
/// Framing happens on the raw bytes rather than on decoded text: a read can
/// land in the middle of a multi-byte character, and decoding each chunk on
/// its own would corrupt it. Byte `0x0A` cannot occur inside a UTF-8 sequence,
/// so splitting first and decoding whole lines afterwards is always safe.
///
/// A line past the limit is reported once, as [`Overlong`] with the start of
/// the line, and the rest of it is thrown away as it arrives; framing picks
/// up again after its newline, so one oversized event costs only itself.
/// Each byte is searched for a newline once however many reads a line spans.
#[derive(Debug)]
pub struct Lines {
    buffer: Vec<u8>,
    start: usize,
    scanned: usize,
    limit: usize,
    discarding: bool,
}

impl Lines {
    pub fn new(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            scanned: 0,
            limit,
            discarding: false,
        }
    }

    pub fn extend(&mut self, chunk: &[u8]) {
        let chunk = if self.discarding {
            match chunk.iter().position(|byte| *byte == NEWLINE) {
                Some(end) => {
                    self.discarding = false;
                    &chunk[end + 1..]
                }
                None => return,
            }
        } else {
            chunk
        };
        if self.start > 0 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        self.buffer.extend_from_slice(chunk);
    }

    /// The next complete line, or `None` while one is still arriving.
    pub fn take(&mut self) -> Result<Option<String>, Overlong> {
        let searched = self.start + self.scanned;
        let Some(offset) = self.buffer[searched..]
            .iter()
            .position(|byte| *byte == NEWLINE)
        else {
            let length = self.buffer.len() - self.start;
            if length > self.limit {
                let overlong = self.overlong(self.buffer.len());
                self.reset();
                self.discarding = true;
                return Err(overlong);
            }
            self.scanned = length;
            return Ok(None);
        };

        let end = searched + offset;
        let outcome = if end - self.start > self.limit {
            Err(self.overlong(end))
        } else {
            Ok(Some(decode(&self.buffer[self.start..end])))
        };
        self.start = end + 1;
        self.scanned = 0;
        if self.start == self.buffer.len() {
            self.reset();
        }
        outcome
    }

    /// The trailing line of a stream that ended without a final newline.
    pub fn flush(&mut self) -> Result<Option<String>, Overlong> {
        if self.discarding {
            self.discarding = false;
            return Ok(None);
        }
        let outcome = match self.buffer.len() - self.start {
            0 => Ok(None),
            length if length > self.limit => Err(self.overlong(self.buffer.len())),
            _ => Ok(Some(decode(&self.buffer[self.start..]))),
        };
        self.reset();
        outcome
    }

    fn overlong(&self, end: usize) -> Overlong {
        let prefix = &self.buffer[self.start..end.min(self.start + PREFIX)];
        Overlong {
            limit: self.limit,
            prefix: String::from_utf8_lossy(prefix).into_owned(),
        }
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.start = 0;
        self.scanned = 0;
    }
}

fn decode(line: &[u8]) -> String {
    let line = line.strip_suffix(CARRIAGE_RETURN).unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use super::Lines;
    use crate::error::Overlong;

    fn drain(lines: &mut Lines) -> Vec<String> {
        let mut taken = Vec::new();
        while let Ok(Some(line)) = lines.take() {
            taken.push(line);
        }
        taken
    }

    fn overlong(limit: usize, prefix: &str) -> Overlong {
        Overlong {
            limit,
            prefix: prefix.to_string(),
        }
    }

    #[test]
    fn a_line_split_across_two_reads_is_rejoined() {
        let mut lines = Lines::new(1024);
        lines.extend(b"{\"type\":\"as");
        assert!(drain(&mut lines).is_empty());

        lines.extend(b"sistant\"}\n");
        assert_eq!(drain(&mut lines), vec![r#"{"type":"assistant"}"#]);
    }

    #[test]
    fn several_lines_in_one_read_all_come_back() {
        let mut lines = Lines::new(1024);
        lines.extend(b"one\ntwo\nthree\n");
        assert_eq!(drain(&mut lines), vec!["one", "two", "three"]);
    }

    #[test]
    fn a_multibyte_character_split_across_reads_survives() {
        let mut lines = Lines::new(1024);
        let text = "café ☕".as_bytes();
        let (head, tail) = text.split_at(5);

        lines.extend(head);
        assert!(drain(&mut lines).is_empty());
        lines.extend(tail);
        lines.extend(b"\n");

        assert_eq!(drain(&mut lines), vec!["café ☕"]);
    }

    #[test]
    fn carriage_returns_are_stripped_but_empty_lines_survive() {
        let mut lines = Lines::new(1024);
        lines.extend(b"one\r\n\r\ntwo\r\n");
        assert_eq!(drain(&mut lines), vec!["one", "", "two"]);
    }

    #[test]
    fn a_stream_ending_without_a_newline_still_yields_its_last_line() {
        let mut lines = Lines::new(1024);
        lines.extend(b"first\nlast without newline");

        assert_eq!(drain(&mut lines), vec!["first"]);
        assert_eq!(lines.flush(), Ok(Some("last without newline".to_string())));
        assert_eq!(lines.flush(), Ok(None));
    }

    #[test]
    fn an_unterminated_line_past_the_cap_is_reported_once_and_thrown_away() {
        let mut lines = Lines::new(8);
        lines.extend(b"way past the cap");

        assert_eq!(lines.take(), Err(overlong(8, "way past the cap")));
        assert_eq!(lines.take(), Ok(None));

        lines.extend(b" and still going");
        assert_eq!(lines.take(), Ok(None));
        lines.extend(b" until here\nnext\n");
        assert_eq!(drain(&mut lines), vec!["next"]);
    }

    #[test]
    fn a_terminated_line_past_the_cap_is_reported_and_the_next_one_still_read() {
        let mut lines = Lines::new(8);
        lines.extend(b"way past the cap but terminated\nshort\n");

        assert_eq!(
            lines.take(),
            Err(overlong(8, "way past the cap but terminated"))
        );
        assert_eq!(lines.take(), Ok(Some("short".to_string())));
    }

    #[test]
    fn only_the_start_of_an_overlong_line_is_kept() {
        let mut lines = Lines::new(8);
        let line = format!("{{\"type\":\"user\"{}", "x".repeat(10_000));
        lines.extend(line.as_bytes());

        let Err(overlong) = lines.take() else {
            panic!("expected an overlong line");
        };
        assert_eq!(overlong.prefix.len(), 1024);
        assert!(overlong.prefix.starts_with(r#"{"type":"user""#));
    }

    #[test]
    fn a_line_exactly_at_the_cap_is_accepted() {
        let mut lines = Lines::new(8);
        lines.extend(b"12345678\n");

        assert_eq!(lines.take(), Ok(Some("12345678".to_string())));
    }

    #[test]
    fn an_unterminated_trailing_line_past_the_cap_is_refused_on_flush() {
        let mut lines = Lines::new(4);
        lines.extend(b"ok\n");
        assert_eq!(lines.take(), Ok(Some("ok".to_string())));

        lines.extend(b"toolong");
        assert_eq!(lines.flush(), Err(overlong(4, "toolong")));
        assert_eq!(lines.flush(), Ok(None));
    }

    #[test]
    fn the_tail_of_a_discarded_line_is_not_flushed_as_a_line() {
        let mut lines = Lines::new(4);
        lines.extend(b"toolong");
        assert!(lines.take().is_err());
        lines.extend(b"still the same line");

        assert_eq!(lines.flush(), Ok(None));
    }

    #[test]
    fn a_long_line_arriving_a_byte_at_a_time_is_framed_in_linear_time() {
        let mut lines = Lines::new(1024 * 1024);
        let started = Instant::now();

        for _ in 0..128 * 1024 {
            lines.extend(b"x");
            assert_eq!(lines.take(), Ok(None));
        }
        lines.extend(b"\n");

        assert_eq!(
            lines.take().map(|line| line.map(|line| line.len())),
            Ok(Some(128 * 1024))
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "framing took {:?}",
            started.elapsed()
        );
    }
}
