use std::borrow::Cow;

use crate::redact::redact;

const ESCAPE: u8 = 0x1B;
const BELL: u8 = 0x07;
const BACKSLASH: u8 = b'\\';
const TAB: u8 = b'\t';
const LINE_FEED: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';
const DELETE: u8 = 0x7F;
const CONTROL_LEAD: u8 = 0xC2;
const CONTROL_SEQUENCE_INTRODUCER: u8 = 0x9B;

/// Strip terminal control sequences from `text` and redact any credential.
///
/// Text with nothing to remove is returned untouched and unallocated.
pub fn sanitize(text: &str) -> Cow<'_, str> {
    let stripped = strip_control_sequences(text);
    let redacted = match redact(&stripped) {
        Cow::Borrowed(_) => None,
        Cow::Owned(redacted) => Some(redacted),
    };
    match redacted {
        Some(redacted) => Cow::Owned(redacted),
        None => stripped,
    }
}

/// [`sanitize`] over an owned buffer, which is returned untouched when there is
/// nothing to remove.
pub fn sanitize_owned(text: String) -> String {
    let sanitized = match sanitize(&text) {
        Cow::Borrowed(_) => None,
        Cow::Owned(sanitized) => Some(sanitized),
    };
    sanitized.unwrap_or(text)
}

fn strip_control_sequences(text: &str) -> Cow<'_, str> {
    if !text.bytes().any(needs_inspection) {
        return Cow::Borrowed(text);
    }

    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            ESCAPE => index = escape_sequence(bytes, index),
            CARRIAGE_RETURN => {
                output.push('\n');
                index += if bytes.get(index + 1) == Some(&LINE_FEED) {
                    2
                } else {
                    1
                };
            }
            TAB | LINE_FEED => {
                output.push(char::from(bytes[index]));
                index += 1;
            }
            0x00..=0x1F | DELETE => index += 1,
            CONTROL_LEAD => match bytes.get(index + 1) {
                Some(&CONTROL_SEQUENCE_INTRODUCER) => {
                    index = control_sequence(bytes, index + 2).unwrap_or(index + 2);
                }
                Some(0x80..=0x9F) => index += 2,
                _ => {
                    output.push_str(&text[index..index + 2]);
                    index += 2;
                }
            },
            _ => {
                let start = index;
                while index < bytes.len() && !needs_inspection(bytes[index]) {
                    index += 1;
                }
                output.push_str(&text[start..index]);
            }
        }
    }

    Cow::Owned(output)
}

fn needs_inspection(byte: u8) -> bool {
    matches!(byte, 0x00..=0x08 | 0x0B..=0x1F | DELETE | CONTROL_LEAD)
}

/// The end of the sequence introduced by the escape at `index`.
///
/// An unterminated sequence loses only its introducer, leaving the payload as
/// ordinary text, which is what a terminal would show.
fn escape_sequence(bytes: &[u8], index: usize) -> usize {
    match bytes.get(index + 1) {
        Some(b']') => operating_system_command(bytes, index + 2).unwrap_or(index + 2),
        Some(b'P' | b'^' | b'_') => device_control(bytes, index + 2).unwrap_or(index + 2),
        Some(b'[') => control_sequence(bytes, index + 2).unwrap_or(index + 2),
        Some(0x40..=0x5F) => index + 2,
        _ => index + 1,
    }
}

fn operating_system_command(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == BELL {
            return Some(index + 1);
        }
        if bytes[index] == ESCAPE && bytes.get(index + 1) == Some(&BACKSLASH) {
            return Some(index + 2);
        }
        index += 1;
    }
    None
}

fn device_control(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while index < bytes.len() {
        if bytes[index] == ESCAPE && bytes.get(index + 1) == Some(&BACKSLASH) {
            return Some(index + 2);
        }
        index += 1;
    }
    None
}

fn control_sequence(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    while matches!(bytes.get(index), Some(0x30..=0x3F)) {
        index += 1;
    }
    while matches!(bytes.get(index), Some(0x20..=0x2F)) {
        index += 1;
    }
    match bytes.get(index) {
        Some(0x40..=0x7E) => Some(index + 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::REDACTED;

    #[test]
    fn leaves_plain_text_alone() {
        let text = "Compiling example_crate v0.1.0\n    Finished in 4.21s\n";
        assert!(matches!(sanitize(text), Cow::Borrowed(_)));
        assert_eq!(sanitize(text), text);
    }

    #[test]
    fn strips_a_colour_sequence() {
        assert_eq!(
            sanitize("\u{1b}[31merror\u{1b}[0m: failed"),
            "error: failed"
        );
    }

    #[test]
    fn strips_a_cursor_sequence_with_intermediates() {
        assert_eq!(
            sanitize("before\u{1b}[?25l\u{1b}[1;2 qafter"),
            "beforeafter"
        );
    }

    #[test]
    fn strips_a_control_sequence_introduced_by_c1() {
        assert_eq!(sanitize("before\u{9b}31mafter"), "beforeafter");
    }

    #[test]
    fn strips_an_operating_system_command() {
        assert_eq!(sanitize("\u{1b}]0;take over the title\u{7}ok"), "ok");
        assert_eq!(sanitize("\u{1b}]8;;https://evil.test\u{1b}\\ok"), "ok");
    }

    #[test]
    fn strips_device_control_privacy_and_application_strings() {
        assert_eq!(sanitize("a\u{1b}Pq#0;2;0;0;0\u{1b}\\b"), "ab");
        assert_eq!(sanitize("a\u{1b}^private\u{1b}\\b"), "ab");
        assert_eq!(sanitize("a\u{1b}_application\u{1b}\\b"), "ab");
    }

    #[test]
    fn strips_a_lone_escape() {
        assert_eq!(sanitize("a\u{1b}Mb"), "ab");
        assert_eq!(sanitize("a\u{1b}7b"), "a7b");
    }

    #[test]
    fn drops_the_introducer_of_an_unterminated_sequence() {
        assert_eq!(sanitize("\u{1b}]0;never terminated"), "0;never terminated");
        assert_eq!(sanitize("\u{1b}[31"), "31");
    }

    #[test]
    fn normalises_line_endings() {
        assert_eq!(sanitize("one\r\ntwo\rthree\nfour"), "one\ntwo\nthree\nfour");
    }

    #[test]
    fn drops_c0_and_c1_controls_but_keeps_tab_and_newline() {
        assert_eq!(
            sanitize("a\u{0}b\u{8}c\u{b}d\u{c}e\u{1f}f\u{7f}g\u{85}h\ti\nj"),
            "abcdefgh\ti\nj"
        );
    }

    #[test]
    fn keeps_multibyte_text_that_is_not_a_control() {
        let text = "résumé £20 中文 🔐";
        assert_eq!(sanitize(text), text);
    }

    #[test]
    fn redacts_a_credential_hidden_behind_a_control_sequence() {
        assert_eq!(
            sanitize("token=ghp_0123\u{1b}[0m456789abcdefghij"),
            format!("token={REDACTED}")
        );
    }

    #[test]
    fn redacts_and_strips_together() {
        assert_eq!(
            sanitize("\u{1b}[31mfatal\u{1b}[0m: sk-0123456789abcdefghij rejected\r\n"),
            format!("fatal: {REDACTED} rejected\n")
        );
    }

    #[test]
    fn sanitize_owned_keeps_the_original_buffer_when_clean() {
        let text = String::from("nothing to clean here");
        assert_eq!(sanitize_owned(text.clone()), text);
    }
}
