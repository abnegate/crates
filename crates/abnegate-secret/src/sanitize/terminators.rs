use crate::sanitize::BACKSLASH;
use crate::sanitize::BELL;
use crate::sanitize::ESCAPE;
use crate::work;

const STRING_TERMINATOR: [u8; 2] = [ESCAPE, BACKSLASH];

/// Where the last bell and the last string terminator in a text start, so a
/// string sequence opened after every terminator that could close it is known
/// to be unterminated without a scan to the end of the text for each one.
#[derive(Clone, Copy)]
pub(super) struct Terminators {
    bell: Option<usize>,
    string: Option<usize>,
}

impl Terminators {
    pub(super) fn find(bytes: &[u8]) -> Self {
        Self {
            bell: bytes.iter().rposition(|byte| *byte == BELL),
            string: bytes
                .windows(STRING_TERMINATOR.len())
                .rposition(|pair| pair == STRING_TERMINATOR),
        }
    }

    /// The end of an operating system command whose payload starts at `from`:
    /// past the first bell or string terminator.
    pub(super) fn operating_system_command(self, bytes: &[u8], from: usize) -> Option<usize> {
        if !follows(self.bell, from) && !follows(self.string, from) {
            return None;
        }
        first_end(bytes, from, |index| {
            if bytes[index] == BELL {
                Some(index + 1)
            } else {
                string_terminator_end(bytes, index)
            }
        })
    }

    /// The end of a device control, start of string, privacy message or
    /// application program command whose payload starts at `from`: past the
    /// first string terminator.
    pub(super) fn device_control(self, bytes: &[u8], from: usize) -> Option<usize> {
        if !follows(self.string, from) {
            return None;
        }
        first_end(bytes, from, |index| string_terminator_end(bytes, index))
    }
}

fn first_end(bytes: &[u8], from: usize, end_at: impl Fn(usize) -> Option<usize>) -> Option<usize> {
    let end = (from..bytes.len()).find_map(end_at);
    work::scanned(end.unwrap_or(bytes.len()) - from);
    end
}

fn follows(terminator: Option<usize>, from: usize) -> bool {
    terminator.is_some_and(|start| start >= from)
}

fn string_terminator_end(bytes: &[u8], index: usize) -> Option<usize> {
    bytes[index..]
        .starts_with(&STRING_TERMINATOR)
        .then_some(index + STRING_TERMINATOR.len())
}
