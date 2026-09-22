use std::borrow::Cow;

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
/// entropy by nature, and `JWT_SECRET=correct-horse-battery-staple` is a
/// credential however it reads.
const NAMED_SECRET_KEY_WORDS: &[&str] = &[
    "accesskey",
    "apikey",
    "credential",
    "encryptionkey",
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

/// Words that introduce a credential without an assignment, as
/// `Authorization: Bearer <token>` does.
const SECRET_INTRODUCERS: &[&str] = &["bearer", "basic", "token", "password", "passwd", "secret"];

/// Shortest run redacted when a key word names it outright.
const MINIMUM_NAMED_LENGTH: usize = 6;

const PEM_BEGIN: &str = "-----BEGIN ";
const PEM_END: &str = "-----END ";
const PEM_PRIVATE: &str = "PRIVATE KEY-----";

#[derive(Clone, Copy)]
enum Charset {
    /// `[A-Za-z0-9_.:/+-]`
    Token,
    /// `[A-Za-z0-9_-]`
    Word,
    /// `[A-Za-z0-9+/_-]`
    Encoded,
}

impl Charset {
    fn contains(self, byte: u8) -> bool {
        byte.is_ascii_alphanumeric()
            || match self {
                Self::Token => matches!(byte, b'_' | b'.' | b':' | b'/' | b'+' | b'-'),
                Self::Word => matches!(byte, b'_' | b'-'),
                Self::Encoded => matches!(byte, b'+' | b'/' | b'_' | b'-'),
            }
    }
}

/// A credential family recognised by its prefix.
struct Credential {
    prefix: &'static str,
    body: Charset,
    minimum: usize,
}

const CREDENTIALS: &[Credential] = &[
    Credential {
        prefix: "npm_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "ghp_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "gho_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "ghu_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "ghs_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "ghr_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "github_pat_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "sk-",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "xoxb-",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "xoxa-",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "xoxp-",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "xoxr-",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "xoxs-",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "AKIA",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "lin_api_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "sntryu_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "sntrys_",
        body: Charset::Token,
        minimum: 8,
    },
    Credential {
        prefix: "glpat-",
        body: Charset::Word,
        minimum: 16,
    },
    Credential {
        prefix: "sk_live_",
        body: Charset::Encoded,
        minimum: 16,
    },
    Credential {
        prefix: "AIzaSy",
        body: Charset::Word,
        minimum: 20,
    },
];

/// Replace every credential in `text` with [`REDACTED`].
///
/// Text with nothing to redact is returned untouched and unallocated.
pub fn redact(text: &str) -> Cow<'_, str> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;

    while index < text.len() {
        match secret_at(text, index) {
            Some(end) => {
                spans.push((index, end));
                index = end;
            }
            None => index += 1,
        }
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

fn secret_at(text: &str, index: usize) -> Option<usize> {
    private_key_at(text, index)
        .or_else(|| credential_at(text, index))
        .or_else(|| json_web_token_at(text, index))
        .or_else(|| url_password_at(text, index))
        .or_else(|| encoded_at(text, index))
        .or_else(|| named_at(text, index))
}

/// The body of a PEM private key, from its BEGIN line to the end of its END
/// line. Nothing in the block carries a prefix or an assignment, and the base64
/// is newline-wrapped, so the run scanners never see it whole.
fn private_key_at(text: &str, index: usize) -> Option<usize> {
    let rest = text.get(index..)?;
    if !rest.starts_with(PEM_BEGIN) {
        return None;
    }
    let line = rest.find('\n').unwrap_or(rest.len());
    if !rest[..line].contains(PEM_PRIVATE) {
        return None;
    }
    // Closing on the first END marker lets an intervening one -- an END line
    // for some other label, sitting in the same output -- cut the redaction
    // short and leave the rest of the key material standing. Close on the label
    // this block opened with.
    let label = rest[PEM_BEGIN.len()..line].trim_end_matches('\r');
    let closing = format!("{PEM_END}{label}");
    // The marker has to be the whole line. Matching it anywhere lets a line
    // that merely starts with it -- `-----END X----- and then some` -- close
    // the block, which is the same escape one spelling further on.
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

/// The password in a `scheme://user:password@host` URL.
///
/// The run is preceded by a colon, so `assigned_to_secret` looks back over the
/// *username* for a key word and finds none: `postgres://user:<password>@db`
/// reads as an assignment to `user`.
fn url_password_at(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if index == 0 || bytes[index - 1] != b':' {
        return None;
    }

    let mut start = index - 1;
    while start > 0 && !matches!(bytes[start - 1], b'/' | b'@' | b' ' | b'\t' | b'\n') {
        start -= 1;
    }
    if !text[..start].ends_with("://") {
        return None;
    }

    let mut end = index;
    while end < bytes.len() && !matches!(bytes[end], b'@' | b'/' | b' ' | b'\t' | b'\n') {
        end += 1;
    }
    (end > index && end < bytes.len() && bytes[end] == b'@').then_some(end)
}

/// A run that a key word names outright, whatever it looks like.
fn named_at(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if index > 0 && Charset::Token.contains(bytes[index - 1]) {
        return None;
    }

    let end = run(bytes, index, Charset::Token);
    if end - index < MINIMUM_NAMED_LENGTH {
        return None;
    }
    (assigned_to(bytes, index, NAMED_SECRET_KEY_WORDS) || introduced_at(bytes, index))
        .then_some(end)
}

/// Whether the word immediately before `index`, separated by spaces rather than
/// an assignment, introduces a credential.
fn introduced_at(bytes: &[u8], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 && matches!(bytes[cursor - 1], b' ' | b'\t') {
        cursor -= 1;
    }
    if cursor == index || cursor == 0 {
        return false;
    }

    let end = cursor;
    let limit = end.saturating_sub(KEY_LOOKBEHIND);
    let mut start = end;
    while start > limit && bytes[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }

    let word: String = bytes[start..end]
        .iter()
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect();
    SECRET_INTRODUCERS.contains(&word.as_str())
}

fn credential_at(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if index > 0 && bytes[index - 1].is_ascii_alphanumeric() {
        return None;
    }

    CREDENTIALS.iter().find_map(|credential| {
        let prefix = credential.prefix.as_bytes();
        let body = index + prefix.len();
        if body > bytes.len() || !bytes[index..body].eq_ignore_ascii_case(prefix) {
            return None;
        }
        let end = run(bytes, body, credential.body);
        (end - body >= credential.minimum).then_some(end)
    })
}

fn json_web_token_at(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if index > 0 && Charset::Word.contains(bytes[index - 1]) {
        return None;
    }
    if !bytes[index..].starts_with(JSON_WEB_TOKEN_PREFIX) {
        return None;
    }

    let header = index + JSON_WEB_TOKEN_PREFIX.len();
    let mut end = run(bytes, header, Charset::Word);
    if end - header < JSON_WEB_TOKEN_SEGMENT {
        return None;
    }

    for _ in 0..2 {
        if bytes.get(end) != Some(&b'.') {
            return None;
        }
        let segment = run(bytes, end + 1, Charset::Word);
        if segment - (end + 1) < JSON_WEB_TOKEN_SEGMENT {
            return None;
        }
        end = segment;
    }

    Some(end)
}

fn encoded_at(text: &str, index: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if index > 0 && Charset::Encoded.contains(bytes[index - 1]) {
        return None;
    }

    let end = run(bytes, index, Charset::Encoded);
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
    while start > limit && Charset::Word.contains(bytes[start - 1]) {
        start -= 1;
    }

    let key: String = bytes[start..end]
        .iter()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect();
    words.iter().any(|word| key.contains(word))
}

fn run(bytes: &[u8], from: usize, charset: Charset) -> usize {
    let mut index = from;
    while index < bytes.len() && charset.contains(bytes[index]) {
        index += 1;
    }
    index
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
    use super::redact;

    /// Each of these reached stored tool output whole: the value carries no
    /// vendor prefix, so it was left to the entropy scanner, which needs an
    /// assignment whose key names a secret. A bearer header is introduced by a
    /// space, a URL password's key is the username, and a PEM body is wrapped
    /// across lines.
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
    fn a_connection_string_password_does_not_survive() {
        let redacted =
            redact("DATABASE_URL=postgres://application:hunter2seventeen@db:5432/manager");
        assert!(!redacted.contains("hunter2seventeen"), "{redacted}");
        assert!(
            redacted.contains("postgres://application:") && redacted.contains("@db:5432/manager"),
            "the rest of the URL should stay legible: {redacted}"
        );
    }

    /// Closing on the first END marker in the text lets an END line for some
    /// other label -- which the same tool output can carry, next to the key or
    /// inside it -- end the redaction early and leave the rest standing.
    /// One spelling further on: a line that merely *starts* with the closing
    /// marker is not the closing line, and must not end the redaction.
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

    /// The relaxation must not start eating ordinary output. `key` and `auth`
    /// are deliberately not treated as naming a credential outright, and a
    /// value has to be assigned or introduced to be redacted at all.
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
        ] {
            let redacted = redact(line);
            assert_eq!(redacted, line, "ordinary output was redacted: {redacted}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each sample sits in prose rather than an assignment. `export TOKEN=<value>`
    /// is redacted by the key word alone, whatever the value looks like, so the
    /// whole prefix table could be emptied with the assertion still holding --
    /// seven of these families were in fact removable with the suite green.
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
            "AKIA0123456789ABCDEF",
            "glpat-0123456789abcdefghij",
            "sk_live_0123456789abcdefghij",
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
    fn ignores_a_short_credential_body() {
        let text = "sk-short";
        assert!(matches!(redact(text), Cow::Borrowed(_)));
    }
}
