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
    pub(super) fn new(bytes: &[u8]) -> Self {
        Self {
            bell: work::rfind(bytes, |index| bytes[index] == BELL),
            string: work::rfind(bytes, |index| {
                bytes[index..].starts_with(&STRING_TERMINATOR)
            }),
        }
    }

    /// The end of an operating system command whose payload starts at `from`:
    /// past the first bell or string terminator.
    pub(super) fn operating_system_command(self, bytes: &[u8], from: usize) -> Option<usize> {
        if !follows(self.bell, from) && !follows(self.string, from) {
            return None;
        }
        work::find_map(bytes, from, |index| {
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
        work::find_map(bytes, from, |index| string_terminator_end(bytes, index))
    }
}

fn follows(terminator: Option<usize>, from: usize) -> bool {
    terminator.is_some_and(|start| start >= from)
}

fn string_terminator_end(bytes: &[u8], index: usize) -> Option<usize> {
    bytes[index..]
        .starts_with(&STRING_TERMINATOR)
        .then_some(index + STRING_TERMINATOR.len())
}
