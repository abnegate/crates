mod character_set;
mod credential;

use std::borrow::Cow;
use std::ops::Range;

use crate::redact::character_set::CharacterSet;
use crate::redact::credential::CREDENTIALS;
use crate::redact::credential::Credential;
use crate::work;

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
///
/// A key matches when it contains one of these once its separators are
/// dropped, so `--oauth2-bearer=<token>` reads as `oauth2bearer` and names
/// the bearer credential as `--bearer=<token>` does.
const NAMED_SECRET_KEY_WORDS: &[&str] = &[
    "accesskey",
    "apikey",
    BEARER,
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

/// The authorization scheme that introduces a credential in a header and as a
/// flag, as `Authorization: Bearer <token>` and `--oauth2-bearer <token>` do,
/// and that names the credential assigned to it, as `--oauth2-bearer=<token>`
/// does.
const BEARER: &str = "bearer";

/// The authorization scheme that introduces a credential in a header, as
/// `Authorization: Basic <credential>` does, but as a flag such as `--basic`
/// switches the scheme on instead.
const BASIC: &str = "basic";

/// Authorization schemes that introduce a credential without an assignment.
const SCHEME_INTRODUCERS: &[&str] = &[BEARER, BASIC];

/// Words that name the credential they introduce without an assignment, as
/// `--password <value>` and `invalid token <value>` do.
const NAMING_INTRODUCERS: &[&str] = &["token", "password", "passwd", "secret"];

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
const FLAG: u8 = b'-';

/// Bytes that separate an unquoted value from what follows it, or close the
/// structure it sits in, when they are not part of the value itself.
const VALUE_DELIMITERS: &[u8] = b",;&)]}";
const OPENING_BRACKETS: &[u8] = b"([{";
const CLOSING_BRACKETS: &[u8] = b")]}";

const PEM_BEGIN: &str = "-----BEGIN ";
const PEM_END: &str = "-----END ";
const PEM_DASHES: &str = "-----";
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

/// Whether any scanner could match at `index`: every one starts on a token
/// byte other than a colon, but for the URL password scanner, which starts
/// after a colon inside userinfo, and the named value scanner, which starts on
/// any byte that can open a value after an assignment, a space or a quote.
fn may_start_secret(bytes: &[u8], index: usize, userinfo: Option<usize>) -> bool {
    let byte = bytes[index];
    let previous = index.checked_sub(1).map(|previous| bytes[previous]);
    (byte != b':' && CharacterSet::Token.contains(byte))
        || (userinfo.is_some() && previous == Some(b':'))
        || (opens_value(byte)
            && previous.is_some_and(|previous| {
                matches!(previous, b'=' | b':' | b' ' | b'\t') || QUOTES.contains(&previous)
            }))
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
///
/// The label is read only as far as the dashes that close it or the end of
/// its line, whichever comes first. Either comes no later than the next BEGIN
/// marker, so repeated markers are each read once.
fn private_key_at(text: &str, index: usize) -> Option<usize> {
    let rest = text.get(index..)?;
    let label = opening_label(rest.strip_prefix(PEM_BEGIN)?);
    if !label.contains(PEM_PRIVATE) {
        return None;
    }
    let closing = format!("{PEM_END}{label}");
    let bytes = rest.as_bytes();
    let end = closes_at(rest, &closing).map_or(rest.len(), |at| {
        work::find(bytes, at, |index| bytes[index] == b'\n').unwrap_or(rest.len())
    });
    Some(index + end)
}

/// The label at the start of `text`: up to the dashes that close it or the end
/// of its line, whichever comes first, less the carriage return of a CRLF line.
fn opening_label(text: &str) -> &str {
    let bytes = text.as_bytes();
    let end = work::find(bytes, 0, |index| {
        bytes[index] == b'\n' || bytes[index..].starts_with(PEM_DASHES.as_bytes())
    });
    match end {
        Some(newline) if bytes[newline] == b'\n' => {
            let line = &text[..newline];
            line.strip_suffix('\r').unwrap_or(line)
        }
        Some(dashes) => &text[..dashes],
        None => text,
    }
}

/// Where `closing`, and the dashes that close its label, occur as a complete
/// line rather than as a prefix of one.
fn closes_at(text: &str, closing: &str) -> Option<usize> {
    work::occurrences(text, closing).find(|at| {
        let after = &text[at + closing.len()..];
        let after = after.strip_prefix(PEM_DASHES).unwrap_or(after);
        after.is_empty() || after.starts_with('\n') || after.starts_with('\r')
    })
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

    let end = work::find(bytes, index, |at| {
        URL_USERINFO_DELIMITERS.contains(&bytes[at])
    })
    .unwrap_or(bytes.len());
    if end > index && bytes.get(end) == Some(&URL_USERINFO_END) {
        return Some(end);
    }
    *userinfo = None;
    None
}

/// A value that a key word names outright, whatever it looks like, or that a
/// word such as `Bearer` or a flag such as `--password` introduces.
///
/// A named value opened by a quote runs to the closing quote, or to the end of
/// the line when there is none; one that is not quoted runs to the
/// [end of the value](unquoted_end).
///
/// Only a run that starts after a colon can overlap the run before it, so one
/// is scanned only once an assignment is known to name it; a scan that finds
/// the value is skipped past, and one that does not stops short.
fn named_at(bytes: &[u8], index: usize) -> Option<usize> {
    let first = *bytes.get(index)?;
    let previous = bytes[index.checked_sub(1)?];
    if !opens_value(first) {
        return None;
    }
    match previous {
        b'=' | b':' => assigned_value_at(bytes, index, None),
        b' ' | b'\t' => assigned_value_at(bytes, index, None)
            .or_else(|| introduced_value_at(bytes, index, None)),
        quote if QUOTES.contains(&quote) => assigned_value_at(bytes, index, Some(quote))
            .or_else(|| introduced_value_at(bytes, index, Some(quote))),
        _ => None,
    }
}

/// Whether a value can start with `byte`. A colon or an equals sign cannot, so
/// that `std::password::hashing` and `password == other` assign nothing.
fn opens_value(byte: u8) -> bool {
    !byte.is_ascii_whitespace() && !QUOTES.contains(&byte) && !matches!(byte, b':' | b'=')
}

fn assigned_value_at(bytes: &[u8], index: usize, quote: Option<u8>) -> Option<usize> {
    if !assigned_to(bytes, index, NAMED_SECRET_KEY_WORDS) {
        return None;
    }
    let end = value_end(bytes, index, quote);
    (end - index >= MINIMUM_NAMED_LENGTH).then_some(end)
}

/// A value introduced by the word before it rather than assigned.
///
/// After a flag that names a credential or the bearer scheme, as `--password
/// letmein` and `--oauth2-bearer <token>` do, the value is redacted whatever
/// it looks like, unless it is another flag; `--basic` introduces nothing.
/// After a word in prose, as in `invalid token <value>`, it is redacted only
/// when it carries a digit or is long and does not read as words, so that
/// `invalid token expired` and `invalid token abcdefgh` both stay; an
/// Authorization header's scheme redacts whatever follows it.
fn introduced_value_at(bytes: &[u8], index: usize, quote: Option<u8>) -> Option<usize> {
    let opening = index - usize::from(quote.is_some());
    let word = word_before(bytes, opening)?;
    let introducer = &bytes[word.clone()];
    let naming = is_one_of(introducer, NAMING_INTRODUCERS);
    let flag = word.start > 0 && bytes[word.start - 1] == FLAG;

    let end = if flag {
        let bearer = introducer.eq_ignore_ascii_case(BEARER.as_bytes());
        let another_flag = quote.is_none() && bytes[index] == FLAG;
        if !(naming || bearer) || another_flag {
            return None;
        }
        value_end(bytes, index, quote)
    } else {
        let scheme = is_one_of(introducer, SCHEME_INTRODUCERS);
        if quote.is_some() || (!naming && !scheme) {
            return None;
        }
        let end = CharacterSet::Token.run(bytes, index);
        let authorization = assigned_to(bytes, word.start, AUTHORIZATION_KEY_WORDS);
        if !authorization && !is_secret_shaped(&bytes[index..end]) {
            return None;
        }
        end
    };
    (end - index >= MINIMUM_NAMED_LENGTH).then_some(end)
}

fn value_end(bytes: &[u8], from: usize, quote: Option<u8>) -> usize {
    quote.map_or_else(
        || unquoted_end(bytes, from),
        |quote| quoted_end(bytes, from, quote),
    )
}

/// Where the quoted value starting at `from` ends: its closing `quote`, or the
/// end of the line when the quote is never closed.
fn quoted_end(bytes: &[u8], from: usize, quote: u8) -> usize {
    let mut escaped = false;
    work::find(bytes, from, |index| {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            return false;
        }
        escaped = byte == ESCAPE;
        matches!(byte, b'\n' | b'\r') || byte == quote
    })
    .unwrap_or(bytes.len())
}

/// Where the unquoted value starting at `from` ends: at whitespace or a quote,
/// or at a [delimiter](VALUE_DELIMITERS) that ends it rather than belongs to
/// it.
///
/// A delimiter ends the value when the end, whitespace, a quote, another
/// delimiter or a `name=` pair follows it, and a closing bracket belongs to the
/// value when the value opened it: `password=a,b` and `P@ss)w0rd` stay whole,
/// while `f(password=abc)` and `api_key=abc&format=json` stop at the `)` and
/// the `&`.
fn unquoted_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 0usize;
    work::find(bytes, from, |index| {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || QUOTES.contains(&byte) {
            return true;
        }
        if OPENING_BRACKETS.contains(&byte) {
            depth += 1;
        } else if depth > 0 && CLOSING_BRACKETS.contains(&byte) {
            depth -= 1;
        } else if depth == 0 && VALUE_DELIMITERS.contains(&byte) && ends_value(bytes, index + 1) {
            return true;
        }
        false
    })
    .unwrap_or(bytes.len().max(from))
}

fn ends_value(bytes: &[u8], after: usize) -> bool {
    let Some(&next) = bytes.get(after) else {
        return true;
    };
    next.is_ascii_whitespace()
        || QUOTES.contains(&next)
        || VALUE_DELIMITERS.contains(&next)
        || begins_pair(bytes, after)
}

/// Whether a `name=` or `name:` pair starts at `from`.
fn begins_pair(bytes: &[u8], from: usize) -> bool {
    let name = CharacterSet::Word.run(bytes, from);
    name > from && matches!(bytes.get(name), Some(b'=' | b':'))
}

/// The alphabetic word immediately before `index`, separated from it by
/// spaces rather than an assignment.
fn word_before(bytes: &[u8], index: usize) -> Option<Range<usize>> {
    let end = work::run_back(bytes, 0..index, |byte| matches!(byte, b' ' | b'\t'));
    if end == index || end == 0 {
        return None;
    }

    let start = work::run_back(bytes, end.saturating_sub(KEY_LOOKBEHIND)..end, |byte| {
        byte.is_ascii_alphabetic()
    });
    (start < end).then_some(start..end)
}

fn is_one_of(word: &[u8], words: &[&str]) -> bool {
    words
        .iter()
        .any(|candidate| word.eq_ignore_ascii_case(candidate.as_bytes()))
}

/// Whether a run reads as a credential rather than as prose: it carries a
/// digit, or it is long and does not read as words.
fn is_secret_shaped(candidate: &[u8]) -> bool {
    work::any(candidate, |byte| byte.is_ascii_digit())
        || (candidate.len() >= MINIMUM_ALPHANUMERIC_RUN && !reads_as_words(candidate))
}

/// Whether every hyphen- or underscore-separated part is a lowercase,
/// capitalised or uppercase word.
fn reads_as_words(candidate: &[u8]) -> bool {
    let mut first = true;
    let mut lowercase_tail = true;
    let mut uppercase = true;
    work::all(candidate, |byte| {
        if matches!(byte, b'-' | b'_') {
            let cased = lowercase_tail || uppercase;
            (first, lowercase_tail, uppercase) = (true, true, true);
            return cased;
        }
        lowercase_tail &= first || byte.is_ascii_lowercase();
        uppercase &= byte.is_ascii_uppercase();
        first = false;
        byte.is_ascii_alphabetic()
    }) && (lowercase_tail || uppercase)
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
        && work::all(identifier, |byte| {
            byte.is_ascii_uppercase() || byte.is_ascii_digit()
        }))
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
    let skip_padding = |end: usize| {
        work::run_back(bytes, 0..end, |byte| {
            matches!(byte, b' ' | b'\t' | b'"' | b'\'' | b'`')
        })
    };

    let cursor = skip_padding(index);
    if cursor == 0 || !matches!(bytes[cursor - 1], b'=' | b':') {
        return false;
    }

    let end = skip_padding(cursor - 1);
    let start = work::run_back(bytes, end.saturating_sub(KEY_LOOKBEHIND)..end, |byte| {
        CharacterSet::Word.contains(byte)
    });

    let mut buffer = [0u8; KEY_LOOKBEHIND];
    let mut length = 0;
    work::each(&bytes[start..end], |byte| {
        if byte.is_ascii_alphanumeric() {
            buffer[length] = byte.to_ascii_lowercase();
            length += 1;
        }
    });
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
    if !work::any(candidate, |byte| byte.is_ascii_alphabetic())
        || !work::any(candidate, |byte| byte.is_ascii_digit())
    {
        return false;
    }

    let mut counts = [0u32; 128];
    work::each(candidate, |byte| counts[usize::from(byte)] += 1);
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
    work::all(candidate, |byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn longest_alphanumeric_run(candidate: &[u8]) -> usize {
    let mut longest = 0;
    let mut current = 0;
    work::each(candidate, |byte| {
        current = if byte.is_ascii_alphanumeric() {
            current + 1
        } else {
            0
        };
        longest = longest.max(current);
    });
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
            concat!("-H 'Authorization: Bearer ghs_", "notaprefixhere123456'"),
        ] {
            let redacted = redact(line);
            assert!(
                !redacted.contains("sk-live-8f3a91c74b2e6d05a1")
                    && !redacted.contains("AbCdEf0123456789XyZ")
                    && !redacted.contains(concat!("ghs_", "notaprefixhere123456")),
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
    fn a_value_after_a_secret_flag_does_not_survive_whatever_it_looks_like() {
        for (line, expected) in [
            (
                "mysql --password letmein",
                format!("mysql --password {REDACTED}"),
            ),
            (
                "docker login --password SuperSecret",
                format!("docker login --password {REDACTED}"),
            ),
            (
                "mysql --password 12345678",
                format!("mysql --password {REDACTED}"),
            ),
            (
                "mysql --password P@ssw0rd! --host db",
                format!("mysql --password {REDACTED} --host db"),
            ),
            (
                "vault login -token hvs.letmeinplease",
                format!("vault login -token {REDACTED}"),
            ),
            (
                "tool --db-secret correcthorse --verbose",
                format!("tool --db-secret {REDACTED} --verbose"),
            ),
            (
                "docker login --password \"two words\" --username app",
                format!("docker login --password \"{REDACTED}\" --username app"),
            ),
        ] {
            assert_eq!(redact(line), expected);
        }
    }

    #[test]
    fn a_token_after_a_bearer_flag_does_not_survive() {
        let token = concat!("ya29", "a0AfH6SMBx3jd8");
        let dotted = concat!("ya29", ".a0AfH6SMBx3jd8", ".Qk9HVVNfVE9LRU4");
        for (line, expected) in [
            (
                format!("curl --oauth2-bearer {token}"),
                format!("curl --oauth2-bearer {REDACTED}"),
            ),
            (
                format!("curl --oauth2-bearer {token} https://example.com"),
                format!("curl --oauth2-bearer {REDACTED} https://example.com"),
            ),
            (
                String::from("tool --bearer abc123def456"),
                format!("tool --bearer {REDACTED}"),
            ),
            (
                format!("curl -H \"Authorization: Bearer {token}\" https://example.com"),
                format!("curl -H \"Authorization: Bearer {REDACTED}\" https://example.com"),
            ),
            (
                format!("tool --bearer={token}"),
                format!("tool --bearer={REDACTED}"),
            ),
            (
                format!("curl --oauth2-bearer={dotted} https://example.com"),
                format!("curl --oauth2-bearer={REDACTED} https://example.com"),
            ),
            (
                format!("curl --oauth2-bearer=\"{dotted}\" https://example.com"),
                format!("curl --oauth2-bearer=\"{REDACTED}\" https://example.com"),
            ),
        ] {
            assert_eq!(redact(&line), expected);
        }
    }

    /// `bearer` names the credential assigned to it just as `token` does, so
    /// an assignment in prose is judged alike for either: `token bearer=owner`
    /// stays, as `token=owner` does, because the value is too short to be a
    /// credential, while a longer value is redacted whatever it looks like.
    #[test]
    fn a_value_assigned_to_bearer_is_judged_as_one_assigned_to_token() {
        assert_eq!(redact("token bearer=owner"), "token bearer=owner");
        for key in ["token", "bearer"] {
            let short = format!("issued to {key}=owner");
            assert_eq!(redact(&short), short);
            assert_eq!(
                redact(&format!("issued to {key}=account-holder")),
                format!("issued to {key}={REDACTED}")
            );
        }
    }

    #[test]
    fn flags_that_carry_no_credential_are_left_alone() {
        for line in [
            "mysql --user root --password --database application",
            "curl --basic https://example.com",
            "curl --basic https://example.com/status",
            "curl --basic=https://example.com",
            "tool --bearer --verbose",
            "docker login --password-stdin --username application",
        ] {
            assert_eq!(redact(line), line);
        }
    }

    /// A word such as `token` or `password` in prose introduces a credential
    /// only when what follows carries a digit or is long and not a word:
    /// `invalid token abcdefgh` stays, as `invalid token expired` must.
    #[test]
    fn a_value_after_a_secret_word_carries_a_digit_or_is_long() {
        for (line, expected) in [
            (
                "the password 12345678 was rejected",
                format!("the password {REDACTED} was rejected"),
            ),
            ("secret 0000111122223333", format!("secret {REDACTED}")),
            (
                "invalid token AbCdEfGhIjKlMnOpQr in request",
                format!("invalid token {REDACTED} in request"),
            ),
        ] {
            assert_eq!(redact(line), expected);
        }
        for line in ["invalid token abcdefgh", "invalid token expired"] {
            assert_eq!(redact(line), line);
        }
    }

    #[test]
    fn a_named_value_runs_past_symbols_to_the_end_of_the_value() {
        for (line, expected) in [
            (
                "POSTGRES_PASSWORD=P@ssw0rd123",
                format!("POSTGRES_PASSWORD={REDACTED}"),
            ),
            (
                "DB_PASSWORD=a!b2c3d4e5f6g7",
                format!("DB_PASSWORD={REDACTED}"),
            ),
            (
                "SECRET_KEY=django-insecure-#x9!k@2z)w",
                format!("SECRET_KEY={REDACTED}"),
            ),
            (
                "DB_PASSWORD=!Secret99 other=1",
                format!("DB_PASSWORD={REDACTED} other=1"),
            ),
            (
                "password: \"%Secret99\"",
                format!("password: \"{REDACTED}\""),
            ),
            ("API_TOKEN=abc(def)ghi=", format!("API_TOKEN={REDACTED}")),
            (
                "Server=db;Password=P@ss;w0rd!;Database=application",
                format!("Server=db;Password={REDACTED};Database=application"),
            ),
        ] {
            assert_eq!(redact(line), expected);
        }
    }

    #[test]
    fn a_named_value_stops_at_the_structure_around_it() {
        for (line, expected) in [
            (
                "connect(user=application, password=hunter2!x)",
                format!("connect(user=application, password={REDACTED})"),
            ),
            (
                "{password: P@ssw0rd!, user: application}",
                format!("{{password: {REDACTED}, user: application}}"),
            ),
            ("[token=abc123def]", format!("[token={REDACTED}]")),
            (
                "https://api.example.com/v1?api_key=abc!123&format=json",
                format!("https://api.example.com/v1?api_key={REDACTED}&format=json"),
            ),
            (
                "\"DB_PASSWORD=P@ssw0rd123\"",
                format!("\"DB_PASSWORD={REDACTED}\""),
            ),
            (
                "PASSWORD=P@ssw0rd123; echo done",
                format!("PASSWORD={REDACTED}; echo done"),
            ),
        ] {
            assert_eq!(redact(line), expected);
        }
    }

    #[test]
    fn code_that_compares_a_secret_is_left_alone() {
        for line in [
            "if (password==undefined) return;",
            "assert token != expected",
            "fn check(password: &str) -> bool",
        ] {
            assert_eq!(redact(line), line);
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
            "-----BEGIN RSA PRIVATE",
            " KEY-----\n",
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
            "-----BEGIN RSA PRIVATE",
            " KEY-----\n",
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
    fn a_private_key_is_redacted_whatever_follows_its_begin_marker() {
        for key in [
            concat!(
                "-----BEGIN RSA PRIVATE",
                " KEY----- exported by example\n",
                "MIIEowIBAAKCAQEAx4fW1pQ8mJ7kR2vLnT5cYdB3sHgKqZ0uWpXvNfE1aOiCjMlP\n",
                "-----END RSA PRIVATE KEY-----"
            ),
            concat!(
                "-----BEGIN RSA PRIVATE",
                " KEY\r\n",
                "MIIEowIBAAKCAQEAx4fW1pQ8mJ7kR2vLnT5cYdB3sHgKqZ0uWpXvNfE1aOiCjMlP\r\n",
                "-----END RSA PRIVATE KEY"
            ),
        ] {
            assert_eq!(redact(key), REDACTED);
        }
    }

    #[test]
    fn a_private_key_closes_at_its_own_end_marker_among_repeated_begin_markers() {
        let text = concat!(
            "-----BEGIN -----BEGIN EC PRIVATE",
            " KEY-----\n",
            "MHcCAQEEIBkg4LVWM9nuwNSkbEGmRe3cTpy4H3fYyjtBZqTg1sh8oAoGCCqGSM49\n",
            "-----END EC PRIVATE KEY-----\n",
            "done"
        );
        assert_eq!(redact(text), format!("-----BEGIN {REDACTED}\ndone"));
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
            "-----BEGIN RSA PRIVATE",
            " KEY-----\n",
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
    use super::*;

    const LENGTH: usize = 16 * 1024;

    /// Each sample sits in prose rather than an assignment, which would be
    /// redacted by its key word alone and so prove nothing about the prefix.
    #[test]
    fn redacts_every_known_credential_family() {
        let samples = [
            "npm_0123456789abcdefghij",
            concat!("ghp_", "0123456789abcdefghij"),
            concat!("gho_", "0123456789abcdefghij"),
            concat!("ghu_", "0123456789abcdefghij"),
            concat!("ghs_", "0123456789abcdefghij"),
            concat!("ghr_", "0123456789abcdefghij"),
            concat!("github_pat_", "0123456789abcdefghij"),
            concat!("sk-", "0123456789abcdefghij"),
            concat!("lin_api_", "0123456789abcdefghij"),
            concat!("sntryu_", "0123456789abcdefghij"),
            concat!("sntrys_", "0123456789abcdefghij"),
            concat!("xoxb-", "0123456789abcdefghij"),
            concat!("xoxa-", "0123456789abcdefghij"),
            concat!("xoxp-", "0123456789abcdefghij"),
            concat!("xoxr-", "0123456789abcdefghij"),
            concat!("xoxs-", "0123456789abcdefghij"),
            concat!("xapp-", "1-A0123456789-0123456789012-abcdef"),
            concat!("AKIA", "0123456789ABCDEF"),
            concat!("ASIA", "0123456789ABCDEF"),
            concat!("glpat-", "0123456789abcdefghij"),
            concat!("sk_live_", "0123456789abcdefghij"),
            concat!("sk_test_", "0123456789abcdefghij"),
            concat!("rk_live_", "0123456789abcdefghij"),
            concat!("rk_test_", "0123456789abcdefghij"),
            concat!("hf_", "abcdefghijklmnopqrstuvwxyzABCDEFGH"),
            concat!("AIzaSy", "0123456789abcdefghij0123"),
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
        let text = concat!(
            "github=ghp_",
            "0123456789abcdefghij slack=xoxb-",
            "0123456789abcdefghij"
        );
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
        let text = concat!(
            "fatal: bad token ghp_",
            "0123456789abcdefghij, retry with a fresh one"
        );
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
        let text = concat!("clé → ghp_", "0123456789abcdefghij ✅");
        assert_eq!(redact(text), format!("clé → {REDACTED} ✅"));
    }

    #[test]
    fn an_aws_access_key_prefix_needs_the_exact_shape() {
        for text in [
            concat!("ASIA", "0123456789ABCDEFG"),
            concat!("ASIA", "PACIFIC_OPERATIONS_TEAM"),
            "asia0123456789abcdef",
        ] {
            assert_eq!(redact(text), text);
        }
    }

    #[test]
    fn redaction_is_linear_in_the_length_of_minified_json() {
        let unit = "\"k\":1,";
        work::assert_linear(
            LENGTH / unit.len(),
            |repetitions| unit.repeat(repetitions),
            |json| assert!(matches!(redact(json), Cow::Borrowed(_))),
        );
    }

    #[test]
    fn a_url_that_never_closes_its_userinfo_is_scanned_once() {
        let unit = ":1";
        work::assert_linear(
            LENGTH / unit.len(),
            |repetitions| format!("http://x{}", unit.repeat(repetitions)),
            |text| assert!(matches!(redact(text), Cow::Borrowed(_))),
        );
    }

    #[test]
    fn repeated_private_key_markers_are_scanned_once() {
        for marker in ["-----BEGIN ", "-----BEGIN PRIVATE KEY "] {
            work::assert_linear(
                LENGTH / marker.len(),
                |repetitions| marker.repeat(repetitions),
                |text| {
                    redact(text);
                },
            );
        }
    }

    #[test]
    fn named_values_that_stop_short_are_scanned_once() {
        for (prefix, unit) in [
            ("", "password=a,"),
            ("password=", "a,b="),
            ("password=", "=a"),
            ("", "--password -"),
            ("", " \"!"),
        ] {
            work::assert_linear(
                LENGTH / unit.len(),
                |repetitions| format!("{prefix}{}", unit.repeat(repetitions)),
                |text| {
                    redact(text);
                },
            );
        }
    }

    /// Every string of up to six bytes drawn from a letter of each case, the
    /// separators and a digit reads as words exactly when splitting it at its
    /// separators leaves nothing but words.
    #[test]
    fn reads_as_words_agrees_with_splitting_into_words() {
        let split = |candidate: &[u8]| {
            candidate
                .split(|byte| matches!(byte, b'-' | b'_'))
                .all(|word| {
                    let tail = word.get(1..).unwrap_or_default();
                    word.iter().all(u8::is_ascii_alphabetic)
                        && (tail.iter().all(u8::is_ascii_lowercase)
                            || word.iter().all(u8::is_ascii_uppercase))
                })
        };
        let alphabet = b"aZ-_1";
        let mut candidates = vec![Vec::new()];
        for length in 1..=6 {
            let longer: Vec<Vec<u8>> = candidates
                .iter()
                .filter(|candidate| candidate.len() == length - 1)
                .flat_map(|candidate| {
                    alphabet
                        .iter()
                        .map(move |byte| [candidate.as_slice(), &[*byte]].concat())
                })
                .collect();
            candidates.extend(longer);
        }
        for candidate in candidates {
            assert_eq!(
                reads_as_words(&candidate),
                split(&candidate),
                "{:?}",
                String::from_utf8_lossy(&candidate)
            );
        }
    }

    #[test]
    fn ignores_a_short_credential_body() {
        let text = "sk-short";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }
}
