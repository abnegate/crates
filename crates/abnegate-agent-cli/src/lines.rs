//! Newline framing for a child process reading in arbitrary chunks.

use crate::error::Overlong;

/// Splits a byte stream into lines across chunk boundaries.
///
/// Framing happens on the raw bytes rather than on decoded text: a read can
/// land in the middle of a multi-byte character, and decoding each chunk on
/// its own would corrupt it. Byte `0x0A` cannot occur inside a UTF-8 sequence,
/// so splitting first and decoding whole lines afterwards is always safe.
#[derive(Debug)]
pub struct Lines {
    buffer: Vec<u8>,
    limit: usize,
    overlong: bool,
}

impl Lines {
    pub fn new(limit: usize) -> Self {
        Self {
            buffer: Vec::new(),
            limit,
            overlong: false,
        }
    }

    pub fn extend(&mut self, chunk: &[u8]) {
        if !self.overlong {
            self.buffer.extend_from_slice(chunk);
        }
    }

    /// The next complete line, or `None` while one is still arriving.
    pub fn take(&mut self) -> Result<Option<String>, Overlong> {
        if self.overlong {
            return Err(self.overlong());
        }
        let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') else {
            if self.buffer.len() > self.limit {
                return Err(self.overflow());
            }
            return Ok(None);
        };
        if end > self.limit {
            return Err(self.overflow());
        }
        let line = decode(&self.buffer[..end]);
        self.buffer.drain(..=end);
        Ok(Some(line))
    }

    /// The trailing line of a stream that ended without a final newline.
    pub fn flush(&mut self) -> Result<Option<String>, Overlong> {
        if self.overlong {
            return Err(self.overlong());
        }
        if self.buffer.is_empty() {
            return Ok(None);
        }
        if self.buffer.len() > self.limit {
            return Err(self.overflow());
        }
        let line = decode(&self.buffer);
        self.buffer = Vec::new();
        Ok(Some(line))
    }

    fn overflow(&mut self) -> Overlong {
        self.overlong = true;
        self.buffer = Vec::new();
        self.overlong()
    }

    fn overlong(&self) -> Overlong {
        Overlong { limit: self.limit }
    }
}

fn decode(line: &[u8]) -> String {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(line).into_owned()
}

#[cfg(test)]
mod tests {
    use super::Lines;
    use crate::error::Overlong;

    fn drain(lines: &mut Lines) -> Vec<String> {
        let mut taken = Vec::new();
        while let Ok(Some(line)) = lines.take() {
            taken.push(line);
        }
        taken
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
    fn an_unterminated_line_past_the_cap_stops_the_stream() {
        let mut lines = Lines::new(8);
        lines.extend(b"way past the cap with no newline at all");

        assert_eq!(lines.take(), Err(Overlong { limit: 8 }));
        assert_eq!(lines.take(), Err(Overlong { limit: 8 }));
        assert_eq!(lines.flush(), Err(Overlong { limit: 8 }));
    }

    #[test]
    fn a_terminated_line_past_the_cap_stops_the_stream() {
        let mut lines = Lines::new(8);
        lines.extend(b"way past the cap but terminated\n");

        assert_eq!(lines.take(), Err(Overlong { limit: 8 }));
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
        assert_eq!(lines.flush(), Err(Overlong { limit: 4 }));
    }
}
