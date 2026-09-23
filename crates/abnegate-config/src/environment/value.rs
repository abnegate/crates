const SINGLE_QUOTE: char = '\'';
const DOUBLE_QUOTE: char = '"';
const BACKSLASH: char = '\\';
const DOLLAR: char = '$';
const BACKTICK: char = '`';
const LINE_FEED: char = '\n';
const CARRIAGE_RETURN: char = '\r';
const LINE_FEED_ESCAPE: char = 'n';
const CARRIAGE_RETURN_ESCAPE: char = 'r';
const COMMENT: char = '#';
const KEY_PUNCTUATION: char = '_';
const BARE_PUNCTUATION: [char; 9] = ['_', '-', '.', '/', ':', '@', '%', '+', ','];
const SINGLE_QUOTE_BREAKERS: [char; 3] = [SINGLE_QUOTE, LINE_FEED, CARRIAGE_RETURN];
const DOUBLE_QUOTE_ESCAPED: [char; 4] = [BACKSLASH, DOUBLE_QUOTE, DOLLAR, BACKTICK];

/// Whether `key` is a name a POSIX shell can export: an ASCII letter or `_`
/// followed by ASCII letters, digits and `_`.
pub(super) fn is_key(key: &str) -> bool {
    let mut characters = key.chars();

    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == KEY_PUNCTUATION)
        && characters
            .all(|character| character.is_ascii_alphanumeric() || character == KEY_PUNCTUATION)
}

/// Write `value` by the quoting rule documented on
/// [`EnvironmentFile`](crate::EnvironmentFile).
pub(super) fn quote(value: &str) -> String {
    if value.chars().all(is_bare) {
        return value.to_string();
    }

    if !value.contains(SINGLE_QUOTE_BREAKERS) {
        return format!("{SINGLE_QUOTE}{value}{SINGLE_QUOTE}");
    }

    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push(DOUBLE_QUOTE);
    for character in value.chars() {
        let escaped = match character {
            LINE_FEED => LINE_FEED_ESCAPE,
            CARRIAGE_RETURN => CARRIAGE_RETURN_ESCAPE,
            other if DOUBLE_QUOTE_ESCAPED.contains(&other) => other,
            other => {
                quoted.push(other);
                continue;
            }
        };
        quoted.push(BACKSLASH);
        quoted.push(escaped);
    }
    quoted.push(DOUBLE_QUOTE);

    quoted
}

/// Read back a value as written after `=`, the inverse of [`quote`].
///
/// A quoted value may be followed by a `#` comment. A quote that is never
/// closed, or is followed by anything else, leaves the text as it stands.
pub(super) fn unquote(raw: &str) -> String {
    let quoted = match raw.chars().next() {
        Some(SINGLE_QUOTE) => single_quoted(&raw[1..]),
        Some(DOUBLE_QUOTE) => double_quoted(&raw[1..]),
        _ => None,
    };

    quoted.unwrap_or_else(|| raw.to_string())
}

fn is_bare(character: char) -> bool {
    character.is_ascii_alphanumeric() || BARE_PUNCTUATION.contains(&character)
}

fn single_quoted(body: &str) -> Option<String> {
    let end = body.find(SINGLE_QUOTE)?;
    is_trailer(&body[end + 1..]).then(|| body[..end].to_string())
}

fn double_quoted(body: &str) -> Option<String> {
    let mut value = String::with_capacity(body.len());
    let mut characters = body.char_indices();

    while let Some((position, character)) = characters.next() {
        match character {
            DOUBLE_QUOTE => {
                return is_trailer(&body[position + 1..]).then_some(value);
            }
            BACKSLASH => match characters.next() {
                Some((_, LINE_FEED_ESCAPE)) => value.push(LINE_FEED),
                Some((_, CARRIAGE_RETURN_ESCAPE)) => value.push(CARRIAGE_RETURN),
                Some((_, escaped)) if DOUBLE_QUOTE_ESCAPED.contains(&escaped) => {
                    value.push(escaped)
                }
                Some((_, other)) => {
                    value.push(BACKSLASH);
                    value.push(other);
                }
                None => return None,
            },
            other => value.push(other),
        }
    }

    None
}

fn is_trailer(rest: &str) -> bool {
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with(COMMENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE_BREAKS: [char; 2] = [LINE_FEED, CARRIAGE_RETURN];

    #[test]
    fn a_plain_value_is_not_quoted() {
        assert_eq!(quote("simple"), "simple");
        assert_eq!(
            quote("https://example.com:8080/path"),
            "https://example.com:8080/path"
        );
        assert_eq!(quote(""), "");
    }

    #[test]
    fn a_value_the_shell_would_read_is_single_quoted() {
        assert_eq!(quote("with spaces"), "'with spaces'");
        assert_eq!(quote("with#hash"), "'with#hash'");
        assert_eq!(quote("value$var"), "'value$var'");
        assert_eq!(quote("`id`"), "'`id`'");
        assert_eq!(quote("with\"quote"), "'with\"quote'");
        assert_eq!(quote("path\\to\\file"), "'path\\to\\file'");
    }

    #[test]
    fn a_value_a_single_quote_cannot_hold_is_escaped_in_double_quotes() {
        assert_eq!(quote("it's"), "\"it's\"");
        assert_eq!(quote("it's $HOME"), "\"it's \\$HOME\"");
        assert_eq!(quote("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(quote("carriage\rreturn"), "\"carriage\\rreturn\"");
        assert_eq!(quote("a\\b\n\"c\"`d`"), "\"a\\\\b\\n\\\"c\\\"\\`d\\`\"");
    }

    #[test]
    fn a_line_break_never_reaches_the_file() {
        let quoted = quote("x\nADMIN_TOKEN=injected\r\n");

        assert!(!quoted.contains(LINE_BREAKS), "{quoted:?}");
    }

    #[test]
    fn escapes_are_decoded() {
        assert_eq!(unquote("\"a\\nb\""), "a\nb");
        assert_eq!(unquote("\"a\\rb\""), "a\rb");
        assert_eq!(unquote("\"a\\\\nb\""), "a\\nb");
        assert_eq!(unquote("\"\\\"\\$\\`\""), "\"$`");
        assert_eq!(unquote("\"a\\tb\""), "a\\tb");
    }

    #[test]
    fn a_single_quoted_value_is_literal() {
        assert_eq!(unquote("'a\\nb $HOME'"), "a\\nb $HOME");
    }

    #[test]
    fn a_comment_may_follow_a_quoted_value() {
        assert_eq!(unquote("'value' # note"), "value");
        assert_eq!(unquote("\"value\"   #note"), "value");
    }

    #[test]
    fn an_unclosed_or_trailed_quote_is_left_as_written() {
        assert_eq!(unquote("\""), "\"");
        assert_eq!(unquote("'open"), "'open");
        assert_eq!(unquote("\"open\\"), "\"open\\");
        assert_eq!(unquote("'a'b"), "'a'b");
        assert_eq!(unquote("\"a\"b"), "\"a\"b");
    }

    #[test]
    fn long_hostile_values_round_trip() {
        for value in [
            "x\nADMIN_TOKEN=injected",
            "$(touch /tmp/pwned)",
            "`touch /tmp/pwned`",
            "${HOME}' \"; rm -rf / #",
            "trailing backslash\\",
            "\\\"\\'\\n\\r",
            "  padded  ",
            "#leading-hash",
            "=leading-equals",
            "multi\nline\r\nvalue\n",
        ] {
            assert_eq!(unquote(&quote(value)), value, "{value:?}");
        }
    }

    #[test]
    fn a_key_is_a_shell_variable_name() {
        for key in ["KEY", "_KEY", "key_2", "_", "A1"] {
            assert!(is_key(key), "{key:?}");
        }

        for key in [
            "",
            "1KEY",
            "KEY-NAME",
            "KEY NAME",
            "KEY=VALUE",
            "export KEY",
            "KEY\nOTHER",
            "KÉY",
            "#KEY",
        ] {
            assert!(!is_key(key), "{key:?}");
        }
    }
}
