mod terminators;

use std::borrow::Cow;
use std::ops::RangeInclusive;

use crate::redact::redact;
use crate::sanitize::terminators::Terminators;
use crate::work;

const ESCAPE: u8 = 0x1B;
const BELL: u8 = 0x07;
const BACKSLASH: u8 = b'\\';
const TAB: u8 = b'\t';
const LINE_FEED: u8 = b'\n';
const CARRIAGE_RETURN: u8 = b'\r';
const DELETE: u8 = 0x7F;
const CONTROL_SEQUENCE_INTRODUCER: char = '\u{9B}';
const C1_CONTROLS: RangeInclusive<char> = '\u{80}'..='\u{9F}';

/// Unicode's Default_Ignorable_Code_Point property, sorted: characters that
/// render as nothing, or reorder the text around them, so what a reader sees
/// differs from what the text says. A zero-width space or a tag character
/// inside a token hides it from [`redact`], and a right-to-left override
/// disguises code.
const INVISIBLE_CHARACTERS: &[RangeInclusive<char>] = &[
    '\u{00AD}'..='\u{00AD}',
    '\u{034F}'..='\u{034F}',
    '\u{061C}'..='\u{061C}',
    '\u{115F}'..='\u{1160}',
    '\u{17B4}'..='\u{17B5}',
    '\u{180B}'..='\u{180F}',
    '\u{200B}'..='\u{200F}',
    '\u{202A}'..='\u{202E}',
    '\u{2060}'..='\u{206F}',
    '\u{3164}'..='\u{3164}',
    '\u{FE00}'..='\u{FE0F}',
    '\u{FEFF}'..='\u{FEFF}',
    '\u{FFA0}'..='\u{FFA0}',
    '\u{FFF0}'..='\u{FFF8}',
    '\u{1BCA0}'..='\u{1BCA3}',
    '\u{1D173}'..='\u{1D17A}',
    '\u{E0000}'..='\u{E0FFF}',
];

/// Whether each byte is a control, or the UTF-8 lead byte of a C1 control or
/// an [invisible character](INVISIBLE_CHARACTERS), and so must be looked at
/// rather than copied. Tab and line feed are copied.
const INSPECTED: [bool; 256] = inspected_bytes();

/// Strip terminal control sequences and invisible formatting characters from
/// `text` and redact any credential.
///
/// Text with nothing to remove is returned untouched and unallocated. Time is
/// linear in the length of `text`.
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
    if !work::any(text.as_bytes(), needs_inspection) {
        return Cow::Borrowed(text);
    }

    let bytes = text.as_bytes();
    let terminators = Terminators::new(bytes);
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            ESCAPE => index = escape_sequence(bytes, index, terminators),
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
            lead if needs_inspection(lead) => {
                let Some(character) = text[index..].chars().next() else {
                    break;
                };
                let after = index + character.len_utf8();
                index = if character == CONTROL_SEQUENCE_INTRODUCER {
                    control_sequence(bytes, after).unwrap_or(after)
                } else {
                    if !character.is_control() && !is_invisible(character) {
                        output.push(character);
                    }
                    after
                };
            }
            _ => {
                let start = index;
                index = work::run(bytes, index, |byte| !needs_inspection(byte));
                output.push_str(&text[start..index]);
            }
        }
    }

    Cow::Owned(output)
}

fn needs_inspection(byte: u8) -> bool {
    INSPECTED[usize::from(byte)]
}

fn is_invisible(character: char) -> bool {
    let following = INVISIBLE_CHARACTERS.partition_point(|range| *range.start() <= character);
    following
        .checked_sub(1)
        .is_some_and(|range| INVISIBLE_CHARACTERS[range].contains(&character))
}

const fn inspected_bytes() -> [bool; 256] {
    let mut inspected = [false; 256];
    let mut byte = 0;
    while byte < 0x80 {
        inspected[byte] = matches!(byte as u8, 0x00..=0x08 | 0x0B..=0x1F | DELETE);
        byte += 1;
    }
    inspect_leads(&mut inspected, &C1_CONTROLS);
    let mut range = 0;
    while range < INVISIBLE_CHARACTERS.len() {
        inspect_leads(&mut inspected, &INVISIBLE_CHARACTERS[range]);
        range += 1;
    }
    inspected
}

/// Mark the lead byte of every character in `range`: lead bytes rise with the
/// characters they encode, so the first and last character's bound the rest.
const fn inspect_leads(inspected: &mut [bool; 256], range: &RangeInclusive<char>) {
    let mut lead = lead_byte(*range.start());
    while lead <= lead_byte(*range.end()) {
        inspected[lead as usize] = true;
        lead += 1;
    }
}

const fn lead_byte(character: char) -> u8 {
    let mut encoded = [0u8; 4];
    character.encode_utf8(&mut encoded);
    encoded[0]
}

/// The end of the sequence introduced by the escape at `index`.
///
/// An unterminated sequence loses only its introducer, leaving the payload as
/// ordinary text, which is what a terminal would show.
fn escape_sequence(bytes: &[u8], index: usize, terminators: Terminators) -> usize {
    let payload = index + 2;
    match bytes.get(index + 1) {
        Some(b']') => terminators
            .operating_system_command(bytes, payload)
            .unwrap_or(payload),
        Some(b'P' | b'X' | b'^' | b'_') => terminators
            .device_control(bytes, payload)
            .unwrap_or(payload),
        Some(b'[') => control_sequence(bytes, payload).unwrap_or(payload),
        Some(0x20..=0x2F) => intermediate_sequence(bytes, index + 1).unwrap_or(index + 1),
        Some(0x30..=0x7E) => payload,
        _ => index + 1,
    }
}

/// The end of an escape sequence built from intermediate bytes and a final
/// byte, as `ESC ( B` selects a character set.
fn intermediate_sequence(bytes: &[u8], from: usize) -> Option<usize> {
    let index = work::run(bytes, from, |byte| matches!(byte, 0x20..=0x2F));
    match bytes.get(index) {
        Some(0x30..=0x7E) => Some(index + 1),
        _ => None,
    }
}

fn control_sequence(bytes: &[u8], from: usize) -> Option<usize> {
    let parameters = work::run(bytes, from, |byte| matches!(byte, 0x30..=0x3F));
    let index = work::run(bytes, parameters, |byte| matches!(byte, 0x20..=0x2F));
    match bytes.get(index) {
        Some(0x40..=0x7E) => Some(index + 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::REDACTED;

    const TOKEN: &str = concat!("ghp_", "0123456789abcdefghij");

    /// Unicode 17's Default_Ignorable_Code_Point property, listed apart from
    /// the table the implementation strips so that the two cannot drift.
    const DEFAULT_IGNORABLE_CODE_POINTS: &[(char, char)] = &[
        ('\u{00AD}', '\u{00AD}'),
        ('\u{034F}', '\u{034F}'),
        ('\u{061C}', '\u{061C}'),
        ('\u{115F}', '\u{1160}'),
        ('\u{17B4}', '\u{17B5}'),
        ('\u{180B}', '\u{180F}'),
        ('\u{200B}', '\u{200F}'),
        ('\u{202A}', '\u{202E}'),
        ('\u{2060}', '\u{206F}'),
        ('\u{3164}', '\u{3164}'),
        ('\u{FE00}', '\u{FE0F}'),
        ('\u{FEFF}', '\u{FEFF}'),
        ('\u{FFA0}', '\u{FFA0}'),
        ('\u{FFF0}', '\u{FFF8}'),
        ('\u{1BCA0}', '\u{1BCA3}'),
        ('\u{1D173}', '\u{1D17A}'),
        ('\u{E0000}', '\u{E0FFF}'),
    ];

    const LENGTH: usize = 16 * 1024;

    #[test]
    fn strips_every_default_ignorable_code_point() {
        for (start, end) in DEFAULT_IGNORABLE_CODE_POINTS {
            for character in *start..=*end {
                let hidden = format!("fatal: {}{character}{} rejected", &TOKEN[..8], &TOKEN[8..]);
                assert_eq!(
                    sanitize(&hidden),
                    format!("fatal: {REDACTED} rejected"),
                    "U+{:04X} survived",
                    u32::from(character)
                );
            }
        }
    }

    #[test]
    fn redacts_a_credential_split_by_a_tag_character() {
        assert_eq!(
            sanitize(concat!("ghp_", "0123\u{E0020}456789abcdefghij")),
            REDACTED
        );
    }

    #[test]
    fn keeps_text_that_shares_a_lead_byte_with_an_invisible_character() {
        let text = concat!(
            "\u{00A3}20 \u{0376} \u{0628} Ti\u{1EBF}ng Vi\u{1EC7}t ",
            "\u{3053}\u{3093}\u{306B}\u{3061}\u{306F} \u{FF21} ",
            "\u{1F44B} \u{1D11E} \u{F0000}"
        );
        assert_eq!(sanitize(text), text);
    }

    #[test]
    fn unterminated_string_sequences_are_scanned_once() {
        for introducer in ["\u{1b}]", "\u{1b}P", "\u{1b}X", "\u{1b}^", "\u{1b}_"] {
            work::assert_linear(
                LENGTH / introducer.len(),
                |repetitions| introducer.repeat(repetitions),
                |text| assert_eq!(sanitize(text), "", "{introducer:?} left a payload behind"),
            );
        }
    }

    #[test]
    fn a_string_sequence_is_closed_by_a_terminator_after_unterminated_ones() {
        assert_eq!(sanitize("a\u{1b}Pq\u{1b}]0;title\u{7}b"), "aqb");
        assert_eq!(sanitize("a\u{1b}]x\u{1b}Py\u{1b}\\b"), "ab");
        assert_eq!(sanitize("a\u{1b}Pq\u{7}b"), "aqb");
    }

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
        assert_eq!(sanitize("a\u{1b}7b"), "ab");
        assert_eq!(sanitize("a\u{1b}8b"), "ab");
        assert_eq!(sanitize("a\u{1b}cb"), "ab");
    }

    #[test]
    fn strips_an_escape_sequence_with_intermediates() {
        assert_eq!(sanitize("a\u{1b}(Bb"), "ab");
        assert_eq!(sanitize("a\u{1b})0b"), "ab");
        assert_eq!(sanitize("a\u{1b}#8b"), "ab");
        assert_eq!(sanitize("a\u{1b} Fb"), "ab");
    }

    #[test]
    fn strips_a_start_of_string() {
        assert_eq!(sanitize("a\u{1b}Xhidden\u{1b}\\b"), "ab");
    }

    #[test]
    fn strips_bidirectional_and_zero_width_characters() {
        assert_eq!(
            sanitize(concat!(
                "a\u{202E}b\u{202A}c\u{2066}d\u{2069}e\u{200B}f\u{200D}g",
                "\u{2060}h\u{FEFF}i\u{00AD}j\u{061C}k\u{200E}l"
            )),
            "abcdefghijkl"
        );
    }

    #[test]
    fn every_invisible_character_and_c1_control_is_inspected() {
        for range in INVISIBLE_CHARACTERS.iter().chain([&C1_CONTROLS]) {
            for character in range.clone() {
                let mut encoded = [0u8; 4];
                let lead = character.encode_utf8(&mut encoded).as_bytes()[0];
                assert!(needs_inspection(lead), "{character:?} is never inspected");
            }
        }
    }

    #[test]
    fn invisible_characters_are_sorted_and_disjoint() {
        for pair in INVISIBLE_CHARACTERS.windows(2) {
            assert!(
                pair[0].end() < pair[1].start(),
                "{:?} does not precede {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn redacts_a_credential_split_by_an_invisible_character() {
        for hidden in [
            concat!("ghp_", "0123\u{200B}456789abcdefghij"),
            concat!("ghp_", "0123456789\u{202E}abcdefghij"),
        ] {
            assert_eq!(
                sanitize(&format!("fatal: {hidden} rejected")),
                format!("fatal: {REDACTED} rejected")
            );
        }
    }

    #[test]
    fn redacts_a_master_key_in_the_form_it_is_loaded_from() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            sanitize(&format!("EXAMPLE_MASTER_KEY={key}")),
            format!("EXAMPLE_MASTER_KEY={REDACTED}")
        );
    }

    #[test]
    fn keeps_typographic_punctuation() {
        let text = "\u{201C}quoted\u{201D} \u{2014} dash \u{2192} arrow \u{2705} \u{00A3}20";
        assert_eq!(sanitize(text), text);
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
            sanitize(concat!("token=ghp_", "0123\u{1b}[0m456789abcdefghij")),
            format!("token={REDACTED}")
        );
        assert_eq!(sanitize(&format!("{TOKEN}\u{1b}(B")), REDACTED);
    }

    #[test]
    fn redacts_and_strips_together() {
        assert_eq!(
            sanitize(concat!(
                "\u{1b}[31mfatal\u{1b}[0m: sk-",
                "0123456789abcdefghij rejected\r\n"
            )),
            format!("fatal: {REDACTED} rejected\n")
        );
    }

    #[test]
    fn sanitize_owned_keeps_the_original_buffer_when_clean() {
        let text = String::from("nothing to clean here");
        assert_eq!(sanitize_owned(text.clone()), text);
    }
}
