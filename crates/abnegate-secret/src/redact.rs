mod character_set;
mod credential;

use std::borrow::Cow;

use crate::redact::character_set::CharacterSet;
use crate::redact::credential::CREDENTIALS;
use crate::redact::credential::Credential;

/// Stands in for every credential this module removes, and for the body of a
/// [`SecretValue`](crate::SecretValue) that is printed.
pub const REDACTED: &str = "[REDACTED]";

const MINIMUM_ENCODED_LENGTH: usize = 32;
const MINIMUM_ALPHANUMERIC_RUN: usize = 16;
const MINIMUM_DISTINCT_CHARACTERS: usize = 12;
const MINIMUM_ENTROPY_BITS: f64 = 3.5;
const JSON_WEB_TOKEN_PREFIX: &[u8] = b"eyJ";
const JSON_WEB_TOKEN_SEGMENT: usize = 8;
const KEY_LOOKBEHIND: usize = 64;
const SECRET_KEY_WORDS: &[&str] = &[
    "apikey",
    "auth",
    "credential",
    "key",
    "passwd",
    "password",
    "pwd",
    "secret",
    "session",
    "signature",
    "token",
];

/// Key words that name a credential and nothing else. `key` and `auth` are
/// absent on purpose: `--key=main` and `auth=none` are ordinary output, so a
/// value assigned to those still has to look like a secret to be redacted.
/// A value assigned to one of these does not -- a chosen passphrase is low
/// entropy by nature, and a hexadecimal master key reads as a digest.
const NAMED_SECRET_KEY_WORDS: &[&str] = &[
    "accesskey",
    "apikey",
    "credential",
    "encryptionkey",
    "masterkey",
    "passwd",
    "password",
    "privatekey",
    "pwd",
    "secret",
    "secretkey",
    "signature",
    "signingkey",
    "token",
];

/// Key endings that name where a credential is kept rather than the
/// credential itself, as `MASTER_KEY_FILE=/etc/example/master.key` does.
const REFERENCE_KEY_SUFFIXES: &[&str] = &["dir", "directory", "file", "path"];

/// Words that introduce a credential without an assignment, as
/// `Authorization: Bearer <token>` does.
const SECRET_INTRODUCERS: &[&str] = &["bearer", "basic", "token", "password", "passwd", "secret"];

/// Key words whose value is a credential scheme followed by the credential.
const AUTHORIZATION_KEY_WORDS: &[&str] = &["authorization"];

/// The length of the shortest key word, below which no key can contain one.
const SHORTEST_KEY_WORD: usize = shortest_word(&[
    SECRET_KEY_WORDS,
    NAMED_SECRET_KEY_WORDS,
    AUTHORIZATION_KEY_WORDS,
]);

/// Shortest run redacted when a key word names it outright.
const MINIMUM_NAMED_LENGTH: usize = 6;

const QUOTES: &[u8] = b"\"'`";
const ESCAPE: u8 = b'\\';

const PEM_BEGIN: &str = "-----BEGIN ";
const PEM_END: &str = "-----END ";
const PEM_PRIVATE: &str = "PRIVATE KEY";

const URL_SCHEME_SEPARATOR: &[u8] = b"://";
const URL_USERINFO_DELIMITERS: &[u8] = b"/@ \t\n";
const URL_USERINFO_END: u8 = b'@';

const AWS_ACCESS_KEY_PREFIXES: &[&[u8]] = &[b"AKIA", b"ASIA"];
const AWS_ACCESS_KEY_BODY: usize = 16;

/// Replace every credential in `text` with [`REDACTED`].
///
/// Text with nothing to redact is returned untouched and unallocated. Time is
/// linear in the length of `text`.
pub fn redact(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut userinfo: Option<usize> = None;
    let mut index = 0;

    while index < bytes.len() {
        if may_start_secret(bytes, index, userinfo)
            && let Some(end) = secret_at(text, index, &mut userinfo)
        {
            spans.push((index, end));
            userinfo = None;
            index = end;
            continue;
        }
        userinfo = userinfo_after(bytes, index, userinfo);
        index += 1;
    }

    if spans.is_empty() {
        return Cow::Borrowed(text);
    }

    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in spans {
        output.push_str(&text[cursor..start]);
        output.push_str(REDACTED);
        cursor = end;
    }
    output.push_str(&text[cursor..]);
    Cow::Owned(output)
}

/// Whether any scanner could match at `index`: every one but the URL password
/// scanner starts on a token byte other than a colon, and that one starts
/// after a colon inside userinfo.
fn may_start_secret(bytes: &[u8], index: usize, userinfo: Option<usize>) -> bool {
    let byte = bytes[index];
    (byte != b':' && CharacterSet::Token.contains(byte))
        || (userinfo.is_some() && index > 0 && bytes[index - 1] == b':')
}

fn secret_at(text: &str, index: usize, userinfo: &mut Option<usize>) -> Option<usize> {
    let bytes = text.as_bytes();
    private_key_at(text, index)
        .or_else(|| credential_at(bytes, index))
        .or_else(|| aws_access_key_at(bytes, index))
        .or_else(|| json_web_token_at(bytes, index))
        .or_else(|| url_password_at(bytes, index, userinfo))
        .or_else(|| encoded_at(bytes, index))
        .or_else(|| named_at(bytes, index))
}

/// The body of a PEM or PGP private key, from its BEGIN line to the end of its
/// END line. Nothing in the block carries a prefix or an assignment, and the
/// base64 is newline-wrapped, so the run scanners never see it whole.
///
/// The block closes only on an END line for the label it opened with, and only
/// on one that is the whole line: an END line for another label, or one with
/// text after it, must not leave the rest of the key standing.
fn private_key_at(text: &str, index: usize) -> Option<usize> {
    if !text.as_bytes()[index..].starts_with(PEM_BEGIN.as_bytes()) {
        return None;
    }
    let rest = &text[index..];
    let line = rest.find('\n').unwrap_or(rest.len());
    if !rest[..line].contains(PEM_PRIVATE) {
        return None;
    }
    let label = rest[PEM_BEGIN.len()..line].trim_end_matches('\r');
    let closing = format!("{PEM_END}{label}");
    let end = closes_at(rest, &closing).map_or(rest.len(), |at| {
        rest[at..]
            .find('\n')
            .map_or(rest.len(), |newline| at + newline)
    });
    Some(index + end)
}

/// Where `closing` occurs as a complete line, rather than as a prefix of one.
fn closes_at(text: &str, closing: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = text[from..].find(closing) {
        let at = from + offset;
        let after = &text[at + closing.len()..];
        if after.is_empty() || after.starts_with('\n') || after.starts_with('\r') {
            return Some(at);
        }
        from = at + closing.len();
    }
    None
}

/// Where the userinfo of the URL being read starts, for as long as the text
/// read since could still be userinfo.
fn userinfo_after(bytes: &[u8], index: usize, userinfo: Option<usize>) -> Option<usize> {
    if bytes[index..].starts_with(URL_SCHEME_SEPARATOR) {
        return Some(index + URL_SCHEME_SEPARATOR.len());
    }
    userinfo.filter(|start| index < *start || !URL_USERINFO_DELIMITERS.contains(&bytes[index]))
}

/// The password in a `scheme://user:password@host` URL.
///
/// The run is preceded by a colon, so `assigned_to_secret` looks back over the
/// *username* for a key word and finds none: `postgres://user:<password>@db`
/// reads as an assignment to `user`.
///
/// A colon whose password runs into something other than `@` rules out every
/// later colon before that point too, so the userinfo is forgotten and each
/// byte is scanned at most once.
fn url_password_at(bytes: &[u8], index: usize, userinfo: &mut Option<usize>) -> Option<usize> {
    let start = (*userinfo)?;
    let colon = index.checked_sub(1)?;
    if colon < start || bytes[colon] != b':' {
        return None;
    }

    let end = bytes[index..]
        .iter()
        .position(|byte| URL_USERINFO_DELIMITERS.contains(byte))
        .map_or(bytes.len(), |length| index + length);
    if end > index && bytes.get(end) == Some(&URL_USERINFO_END) {
        return Some(end);
    }
    *userinfo = None;
    None
}

/// A run that a key word names outright, whatever it looks like, or that a
/// word such as `Bearer` introduces and that is shaped like a credential.
///
/// A named value opened by a quote runs to the closing quote, or to the end of
/// the line when there is none.
///
/// Only a run that starts after a colon can overlap the run before it, so one
/// is scanned only once an assignment is known to name it; a scan that finds
/// the value is skipped past, and one that does not stops short.
fn named_at(bytes: &[u8], index: usize) -> Option<usize> {
    let first = *bytes.get(index)?;
    if first == b':' || !CharacterSet::Token.contains(first) {
        return None;
    }
    match index.checked_sub(1).map(|previous| bytes[previous]) {
        Some(b':') => assigned_value_at(bytes, index, None),
        Some(quote) if QUOTES.contains(&quote) => assigned_value_at(bytes, index, Some(quote)),
        Some(previous) if CharacterSet::Token.contains(previous) => None,
        _ => assigned_value_at(bytes, index, None).or_else(|| introduced_value_at(bytes, index)),
    }
}

fn assigned_value_at(bytes: &[u8], index: usize, quote: Option<u8>) -> Option<usize> {
    if !assigned_to(bytes, index, NAMED_SECRET_KEY_WORDS) {
        return None;
    }
    let end = quote.map_or_else(
        || CharacterSet::Token.run(bytes, index),
        |quote| quoted_end(bytes, index, quote),
    );
    (end - index >= MINIMUM_NAMED_LENGTH).then_some(end)
}

fn introduced_value_at(bytes: &[u8], index: usize) -> Option<usize> {
    let end = CharacterSet::Token.run(bytes, index);
    if end - index < MINIMUM_NAMED_LENGTH {
        return None;
    }
    let introducer = introducer_before(bytes, index)?;
    (assigned_to(bytes, introducer, AUTHORIZATION_KEY_WORDS)
        || is_secret_shaped(&bytes[index..end]))
    .then_some(end)
}

/// Where the quoted value starting at `from` ends: its closing `quote`, or the
/// end of the line when the quote is never closed.
fn quoted_end(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut index = from;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\n' | b'\r' => return index,
            ESCAPE => index += 2,
            _ if byte == quote => return index,
            _ => index += 1,
        }
    }
    bytes.len()
}

/// The start of the word immediately before `index`, separated by spaces
/// rather than an assignment, when that word introduces a credential.
fn introducer_before(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    while cursor > 0 && matches!(bytes[cursor - 1], b' ' | b'\t') {
        cursor -= 1;
    }
    if cursor == index || cursor == 0 {
        return None;
    }

    let end = cursor;
    let limit = end.saturating_sub(KEY_LOOKBEHIND);
    let mut start = end;
    while start > limit && bytes[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }

    let word = &bytes[start..end];
    SECRET_INTRODUCERS
        .iter()
        .any(|introducer| word.eq_ignore_ascii_case(introducer.as_bytes()))
        .then_some(start)
}

/// Whether a run reads as a credential rather than as prose: it mixes letters
/// with digits, or it is long and does not read as words.
fn is_secret_shaped(candidate: &[u8]) -> bool {
    let letters = candidate.iter().any(u8::is_ascii_alphabetic);
    let digits = candidate.iter().any(u8::is_ascii_digit);
    (letters && digits)
        || (candidate.len() >= MINIMUM_ALPHANUMERIC_RUN && !reads_as_words(candidate))
}

/// Whether every hyphen- or underscore-separated part is a lowercase,
/// capitalised or uppercase word.
fn reads_as_words(candidate: &[u8]) -> bool {
    candidate
        .split(|byte| matches!(byte, b'-' | b'_'))
        .all(|word| {
            let tail = word.get(1..).unwrap_or_default();
            word.iter().all(u8::is_ascii_alphabetic)
                && (tail.iter().all(u8::is_ascii_lowercase)
                    || word.iter().all(u8::is_ascii_uppercase))
        })
}

fn credential_at(bytes: &[u8], index: usize) -> Option<usize> {
    if (index > 0 && bytes[index - 1].is_ascii_alphanumeric())
        || !Credential::may_start(bytes[index])
    {
        return None;
    }
    CREDENTIALS
        .iter()
        .find_map(|credential| credential.end_at(bytes, index))
}

/// An AWS access key ID: a four-letter prefix and sixteen uppercase letters or
/// digits, matched exactly so that prose such as `Asia/Tokyo` is left alone.
fn aws_access_key_at(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && bytes[index - 1].is_ascii_alphanumeric() {
        return None;
    }
    let prefix = AWS_ACCESS_KEY_PREFIXES
        .iter()
        .find(|prefix| bytes[index..].starts_with(prefix))?;
    let body = index + prefix.len();
    let end = body + AWS_ACCESS_KEY_BODY;
    let identifier = bytes.get(body..end)?;
    let terminated = bytes
        .get(end)
        .is_none_or(|byte| !byte.is_ascii_alphanumeric());
    (terminated
        && identifier
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
    .then_some(end)
}

fn json_web_token_at(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && CharacterSet::Word.contains(bytes[index - 1]) {
        return None;
    }
    if !bytes[index..].starts_with(JSON_WEB_TOKEN_PREFIX) {
        return None;
    }

    let header = index + JSON_WEB_TOKEN_PREFIX.len();
    let mut end = CharacterSet::Word.run(bytes, header);
    if end - header < JSON_WEB_TOKEN_SEGMENT {
        return None;
    }

    for _ in 0..2 {
        if bytes.get(end) != Some(&b'.') {
            return None;
        }
        let segment = CharacterSet::Word.run(bytes, end + 1);
        if segment - (end + 1) < JSON_WEB_TOKEN_SEGMENT {
            return None;
        }
        end = segment;
    }

    Some(end)
}

fn encoded_at(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && CharacterSet::Encoded.contains(bytes[index - 1]) {
        return None;
    }

    let end = CharacterSet::Encoded.run(bytes, index);
    if end - index < MINIMUM_ENCODED_LENGTH {
        return None;
    }
    (is_secret_like(&bytes[index..end]) && assigned_to_secret(bytes, index)).then_some(end)
}

/// Whether an assignment immediately before `index` names a credential.
///
/// A run with no recognised prefix is indistinguishable from an integrity
/// hash, a base64 payload or a build identifier, so entropy alone must not
/// redact it: `cargo test` output and lockfiles are full of such runs.
fn assigned_to_secret(bytes: &[u8], index: usize) -> bool {
    assigned_to(bytes, index, SECRET_KEY_WORDS)
}

fn assigned_to(bytes: &[u8], index: usize, words: &[&str]) -> bool {
    let skip_padding = |mut cursor: usize| {
        while cursor > 0 && matches!(bytes[cursor - 1], b' ' | b'\t' | b'"' | b'\'' | b'`') {
            cursor -= 1;
        }
        cursor
    };

    let cursor = skip_padding(index);
    if cursor == 0 || !matches!(bytes[cursor - 1], b'=' | b':') {
        return false;
    }

    let end = skip_padding(cursor - 1);
    let limit = end.saturating_sub(KEY_LOOKBEHIND);
    let mut start = end;
    while start > limit && CharacterSet::Word.contains(bytes[start - 1]) {
        start -= 1;
    }

    let mut buffer = [0u8; KEY_LOOKBEHIND];
    let mut length = 0;
    for byte in bytes[start..end]
        .iter()
        .filter(|byte| byte.is_ascii_alphanumeric())
    {
        buffer[length] = byte.to_ascii_lowercase();
        length += 1;
    }
    let key = &buffer[..length];
    if key.len() < SHORTEST_KEY_WORD {
        return false;
    }

    !REFERENCE_KEY_SUFFIXES
        .iter()
        .any(|suffix| key.ends_with(suffix.as_bytes()))
        && words.iter().any(|word| contains(key, word.as_bytes()))
}

const fn shortest_word(lists: &[&[&str]]) -> usize {
    let mut shortest = usize::MAX;
    let mut list = 0;
    while list < lists.len() {
        let mut word = 0;
        while word < lists[list].len() {
            let length = lists[list][word].len();
            if length < shortest {
                shortest = length;
            }
            word += 1;
        }
        list += 1;
    }
    shortest
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn is_secret_like(candidate: &[u8]) -> bool {
    if is_digest(candidate) || longest_alphanumeric_run(candidate) < MINIMUM_ALPHANUMERIC_RUN {
        return false;
    }
    if !candidate.iter().any(u8::is_ascii_alphabetic) || !candidate.iter().any(u8::is_ascii_digit) {
        return false;
    }

    let mut counts = [0u32; 128];
    for byte in candidate {
        counts[usize::from(*byte)] += 1;
    }
    if counts.iter().filter(|count| **count > 0).count() < MINIMUM_DISTINCT_CHARACTERS {
        return false;
    }

    let length = candidate.len() as f64;
    let entropy: f64 = counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let probability = f64::from(*count) / length;
            -probability * probability.log2()
        })
        .sum();
    entropy >= MINIMUM_ENTROPY_BITS
}

/// Whether the candidate is a hash, commit or UUID rather than a credential.
fn is_digest(candidate: &[u8]) -> bool {
    candidate
        .iter()
        .all(|byte| byte.is_ascii_hexdigit() || *byte == b'-')
}

fn longest_alphanumeric_run(candidate: &[u8]) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for byte in candidate {
        if byte.is_ascii_alphanumeric() {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

#[cfg(test)]
mod shapes_that_carry_no_prefix {
    use super::REDACTED;
    use super::redact;

    #[test]
    fn a_bearer_token_does_not_survive_its_header() {
        for line in [
            "Authorization: Bearer sk-live-8f3a91c74b2e6d05a1",
            "authorization: bearer AbCdEf0123456789XyZ",
            "-H 'Authorization: Bearer ghs_notaprefixhere123456'",
        ] {
            let redacted = redact(line);
            assert!(
                !redacted.contains("sk-live-8f3a91c74b2e6d05a1")
                    && !redacted.contains("AbCdEf0123456789XyZ")
                    && !redacted.contains("ghs_notaprefixhere123456"),
                "{redacted}"
            );
            assert!(
                redacted.to_ascii_lowercase().contains("bearer"),
                "the header should stay legible: {redacted}"
            );
        }
    }

    #[test]
    fn a_basic_credential_does_not_survive_its_header() {
        assert_eq!(
            redact("Authorization: Basic dXNlcjpwYXNz"),
            format!("Authorization: Basic {REDACTED}")
        );
    }

    #[test]
    fn a_credential_introduced_in_prose_is_redacted_when_it_looks_like_one() {
        for (line, expected) in [
            (
                "invalid token AbCdEf0123456789XyZ in request",
                format!("invalid token {REDACTED} in request"),
            ),
            (
                "Bearer Zm9vYmFyYmF6cXV4cXV1eA failed",
                format!("Bearer {REDACTED} failed"),
            ),
            (
                "the password hunter2 was rejected",
                format!("the password {REDACTED} was rejected"),
            ),
        ] {
            assert_eq!(redact(line), expected);
        }
    }

    #[test]
    fn a_connection_string_password_does_not_survive() {
        let redacted =
            redact("DATABASE_URL=postgres://application:hunter2seventeen@db:5432/manager");
        assert!(!redacted.contains("hunter2seventeen"), "{redacted}");
        assert!(
            redacted.contains("postgres://application:") && redacted.contains("@db:5432/manager"),
            "the rest of the URL should stay legible: {redacted}"
        );
    }

    #[test]
    fn an_end_marker_with_trailing_text_does_not_close_a_private_key() {
        let key = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "MIIEowIBAAKCAQEAx4fW1pQ8mJ7kR2vLnT5cYdB3sHgKqZ0uWpXvNfE1aOiCjMlP\n",
            "-----END RSA PRIVATE KEY----- not really, keep reading\n",
            "b2ZuRk9tS3hZd0hqTmRQaVFsY0dYcVJzVHZCa0xtWm5Ob3BBcVJzVHZCa0xtWm4=\n",
            "-----END RSA PRIVATE KEY-----"
        );
        let redacted = redact(key);
        assert!(
            !redacted.contains("b2ZuRk9tS3hZd0hq"),
            "the key material after the partial marker survived: {redacted}"
        );
    }

    #[test]
    fn an_intervening_end_marker_does_not_close_a_private_key() {
        let key = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "MIIEowIBAAKCAQEAx4fW1pQ8mJ7kR2vLnT5cYdB3sHgKqZ0uWpXvNfE1aOiCjMlP\n",
            "-----END CERTIFICATE-----\n",
            "b2ZuRk9tS3hZd0hqTmRQaVFsY0dYcVJzVHZCa0xtWm5Ob3BBcVJzVHZCa0xtWm4=\n",
            "-----END RSA PRIVATE KEY-----"
        );
        let redacted = redact(key);
        assert!(!redacted.contains("MIIEowIBAAKCAQEA"), "{redacted}");
        assert!(
            !redacted.contains("b2ZuRk9tS3hZd0hq"),
            "the key material after the mismatched marker survived: {redacted}"
        );
    }

    #[test]
    fn a_pgp_private_key_block_does_not_survive() {
        let key = concat!(
            "-----BEGIN PGP PRIVATE",
            " KEY BLOCK-----\n",
            "\n",
            "lQOYBGYx2sQBCADm4kHq8vT3yN1pZ0cR7wXjL5fKbA9sUeD2oPiGhM6tVnC3rQzW\n",
            "=Xk3e\n",
            "-----END PGP PRIVATE KEY BLOCK-----"
        );
        assert_eq!(redact(key), REDACTED);
    }

    #[test]
    fn a_private_key_body_does_not_survive() {
        let key = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "MIIEowIBAAKCAQEAx4fW1pQ8mJ7kR2vLnT5cYdB3sHgKqZ0uWpXvNfE1aOiCjMlP\n",
            "b2ZuRk9tS3hZd0hqTmRQaVFsY0dYcVJzVHZCa0xtWm5Ob3BBcVJzVHZCa0xtWm4=\n",
            "-----END RSA PRIVATE KEY-----"
        );
        let redacted = redact(key);
        assert!(!redacted.contains("MIIEowIBAAKCAQEA"), "{redacted}");
        assert!(!redacted.contains("b2ZuRk9tS3hZd0hq"), "{redacted}");
    }

    #[test]
    fn a_master_key_does_not_survive_in_any_spelling() {
        let key = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        for (line, expected) in [
            (
                format!("EXAMPLE_MASTER_KEY={key}"),
                format!("EXAMPLE_MASTER_KEY={REDACTED}"),
            ),
            (
                format!("master_key: {key}"),
                format!("master_key: {REDACTED}"),
            ),
            (format!("masterKey={key}"), format!("masterKey={REDACTED}")),
            (
                format!("export EXAMPLE_MASTER_KEY=\"{key}\""),
                format!("export EXAMPLE_MASTER_KEY=\"{REDACTED}\""),
            ),
            (
                format!("signing_key={key}"),
                format!("signing_key={REDACTED}"),
            ),
            (
                format!("private_key={key}"),
                format!("private_key={REDACTED}"),
            ),
        ] {
            assert_eq!(redact(&line), expected);
        }
    }

    #[test]
    fn a_key_file_path_is_not_a_key() {
        for line in [
            "EXAMPLE_MASTER_KEY_FILE=/home/example/.example/master.key",
            "POSTGRES_PASSWORD_FILE=/run/secrets/postgres",
            "signing_key_path: /etc/example/signing.pem",
        ] {
            assert_eq!(redact(line), line);
        }
    }

    #[test]
    fn a_value_assigned_with_a_bare_colon_does_not_survive() {
        assert_eq!(
            redact("DB_PASSWORD:hunter2seventeen"),
            format!("DB_PASSWORD:{REDACTED}")
        );
    }

    #[test]
    fn a_quoted_passphrase_does_not_survive_past_its_first_word() {
        for (line, expected) in [
            (
                "SECRET=\"correct horse battery staple\"",
                format!("SECRET=\"{REDACTED}\""),
            ),
            (
                "password: 'my pass phrase' # rotated",
                format!("password: '{REDACTED}' # rotated"),
            ),
            (
                "{\"api_key\": \"two words\", \"user\": \"example\"}",
                format!("{{\"api_key\": \"{REDACTED}\", \"user\": \"example\"}}"),
            ),
            (
                "TOKEN=\"never closed\nnext line",
                format!("TOKEN=\"{REDACTED}\nnext line"),
            ),
        ] {
            assert_eq!(redact(line), expected);
        }
    }

    #[test]
    fn a_chosen_passphrase_assigned_to_a_secret_does_not_survive() {
        for line in [
            "JWT_SECRET=correct-horse-battery-staple",
            "ENCRYPTION_KEY: my-dev-passphrase",
            "password = letmein-please",
        ] {
            let redacted = redact(line);
            assert!(
                !redacted.contains("correct-horse-battery-staple")
                    && !redacted.contains("my-dev-passphrase")
                    && !redacted.contains("letmein-please"),
                "{redacted}"
            );
        }
    }

    #[test]
    fn ordinary_output_is_left_alone() {
        for line in [
            "cargo build --key=main --features auth=none",
            "commit 4f9c2b17a3e6d580c1b2a3948f7e6d5c4b3a2918",
            "note: the latest release is 1.9.0",
            "Compiling example_crate v0.1.0 (/Users/x/crates/crates/example-crate)",
            "     Running unittests src/lib.rs (target/debug/deps/example_crate-acf79d25bb672c38)",
            "GET /api/projects 200 in 4ms",
            "warning: unused variable: `token`",
            "https://github.com/abnegate/crates/pull/42",
            "keyword: password",
            "invalid token format in request",
            "the basic example compiles",
            "Bearer authentication failed",
            "the password reset link expired",
            "TZ=Asia/Kolkata",
            "export HF_HUB_CACHE=/var/cache/huggingface",
            "error: std::password::hashing::verify failed",
            "listening on http://localhost:8080 with {\"json\":1}",
        ] {
            let redacted = redact(line);
            assert_eq!(redacted, line, "ordinary output was redacted: {redacted}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    /// A megabyte scanned quadratically takes tens of seconds even optimised;
    /// scanned linearly it takes a few milliseconds optimised and a few tens
    /// unoptimised.
    const LINEAR_BUDGET: Duration = if cfg!(debug_assertions) {
        Duration::from_millis(1000)
    } else {
        Duration::from_millis(100)
    };

    /// Each sample sits in prose rather than an assignment, which would be
    /// redacted by its key word alone and so prove nothing about the prefix.
    #[test]
    fn redacts_every_known_credential_family() {
        let samples = [
            "npm_0123456789abcdefghij",
            "ghp_0123456789abcdefghij",
            "gho_0123456789abcdefghij",
            "ghu_0123456789abcdefghij",
            "ghs_0123456789abcdefghij",
            "ghr_0123456789abcdefghij",
            "github_pat_0123456789abcdefghij",
            "sk-0123456789abcdefghij",
            "lin_api_0123456789abcdefghij",
            "sntryu_0123456789abcdefghij",
            "sntrys_0123456789abcdefghij",
            "xoxb-0123456789abcdefghij",
            "xoxa-0123456789abcdefghij",
            "xoxp-0123456789abcdefghij",
            "xoxr-0123456789abcdefghij",
            "xoxs-0123456789abcdefghij",
            concat!("xapp-", "1-A0123456789-0123456789012-abcdef"),
            "AKIA0123456789ABCDEF",
            concat!("ASIA", "0123456789ABCDEF"),
            "glpat-0123456789abcdefghij",
            "sk_live_0123456789abcdefghij",
            concat!("sk_test_", "0123456789abcdefghij"),
            concat!("rk_live_", "0123456789abcdefghij"),
            concat!("rk_test_", "0123456789abcdefghij"),
            concat!("hf_", "abcdefghijklmnopqrstuvwxyzABCDEFGH"),
            "AIzaSy0123456789abcdefghij0123",
        ];

        for sample in samples {
            let text = format!("the agent echoed {sample} back into its own output");
            assert_eq!(
                redact(&text),
                format!("the agent echoed {REDACTED} back into its own output"),
                "leaked {sample}"
            );
        }
    }

    #[test]
    fn redacts_a_json_web_token() {
        let token = concat!(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.",
            "eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IlpvbmUifQ.",
            "dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXkw"
        );
        let text = format!("Authorization: Bearer {token}");
        assert_eq!(redact(&text), format!("Authorization: Bearer {REDACTED}"));
    }

    #[test]
    fn redacts_a_high_entropy_blob() {
        let text = "SESSION=R8kQz2vXpL7mNc4JwYbTfH1sAe6UgD3iKo9BrVtZxS0";
        assert_eq!(redact(text), format!("SESSION={REDACTED}"));
    }

    #[test]
    fn leaves_an_unassigned_high_entropy_run_alone() {
        for text in [
            "\"integrity\": \"sha512-cca3cea332ad254bb84145f966d19f4879615210346fc92c79a047f23a0d7b3cca\"",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk",
            "target/debug/deps/confinement_tests-28f61f4f60f8bfde",
            "/Users/dev/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/axum-0.8.9",
        ] {
            assert_eq!(redact(text), text, "redaction altered real tool output");
        }
    }

    #[test]
    fn redacts_a_high_entropy_run_assigned_to_a_credential_key() {
        for (text, expected) in [
            (
                "JWT_SECRET=R8kQz2vXpL7mNc4JwYbTfH1sAe6UgD3iKo9BrVtZxS0",
                format!("JWT_SECRET={REDACTED}"),
            ),
            (
                "\"api_key\": \"R8kQz2vXpL7mNc4JwYbTfH1sAe6UgD3iKo9BrVtZxS0\"",
                format!("\"api_key\": \"{REDACTED}\""),
            ),
            (
                "POSTGRES_PASSWORD=R8kQz2vXpL7mNc4JwYbTfH1sAe6UgD3iKo9BrVtZxS0",
                format!("POSTGRES_PASSWORD={REDACTED}"),
            ),
        ] {
            assert_eq!(redact(text), expected);
        }
    }

    #[test]
    fn redacts_every_credential_in_one_line() {
        let text = "github=ghp_0123456789abcdefghij slack=xoxb-0123456789abcdefghij";
        assert_eq!(redact(text), format!("github={REDACTED} slack={REDACTED}"));
    }

    #[test]
    fn leaves_ordinary_prose_alone() {
        let text = "The deployment finished and the reviewer asked for a shorter summary.";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
        assert_eq!(redact(text), text);
    }

    #[test]
    fn leaves_a_git_sha_alone() {
        let text = "Reverted in commit 8d18d30fa1c94b7e2f5a6c0d3e8b1a9f7c2d4e60 on main.";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn leaves_paths_identifiers_and_uuids_alone() {
        let text = concat!(
            "worktree /Users/dev/crates/crates/abnegate-secret/src2 ",
            "branch fix-agent-secret-handling-2026-final ",
            "task 550e8400-e29b-41d4-a716-446655440000"
        );
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn requires_a_boundary_before_a_credential_prefix() {
        let text = "risk-management-review-checklist";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn keeps_the_text_around_a_credential() {
        let text = "fatal: bad token ghp_0123456789abcdefghij, retry with a fresh one";
        assert_eq!(
            redact(text),
            format!("fatal: bad token {REDACTED}, retry with a fresh one")
        );
    }

    #[test]
    fn leaves_multibyte_text_alone() {
        let text = "résumé généré 🔐 avec succès";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }

    #[test]
    fn redacts_a_credential_beside_multibyte_text() {
        let text = "clé → ghp_0123456789abcdefghij ✅";
        assert_eq!(redact(text), format!("clé → {REDACTED} ✅"));
    }

    #[test]
    fn an_aws_access_key_prefix_needs_the_exact_shape() {
        for text in [
            concat!("ASIA", "0123456789ABCDEFG"),
            "ASIAPACIFIC_OPERATIONS_TEAM",
            "asia0123456789abcdef",
        ] {
            assert_eq!(redact(text), text);
        }
    }

    #[test]
    fn redaction_is_linear_in_the_length_of_minified_json() {
        let json = "\"k\":1,".repeat(1024 * 1024 / 6);
        let started = Instant::now();
        let redacted = redact(&json);
        let elapsed = started.elapsed();

        assert!(matches!(redacted, Cow::Borrowed(_)));
        assert!(
            elapsed < LINEAR_BUDGET,
            "a megabyte of minified JSON took {elapsed:?}"
        );
    }

    #[test]
    fn a_url_that_never_closes_its_userinfo_is_scanned_once() {
        let text = format!("http://x{}", ":1".repeat(512 * 1024));
        let started = Instant::now();
        let redacted = redact(&text);
        let elapsed = started.elapsed();

        assert!(matches!(redacted, Cow::Borrowed(_)));
        assert!(
            elapsed < LINEAR_BUDGET,
            "a megabyte of colons after a scheme took {elapsed:?}"
        );
    }

    #[test]
    fn ignores_a_short_credential_body() {
        let text = "sk-short";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }
}
