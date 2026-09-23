use crate::redact::character_set::CharacterSet;

/// A credential family recognised by its prefix.
pub(super) struct Credential {
    prefix: &'static str,
    body: CharacterSet,
    minimum: usize,
}

impl Credential {
    const fn new(prefix: &'static str, body: CharacterSet, minimum: usize) -> Self {
        Self {
            prefix,
            body,
            minimum,
        }
    }

    /// Whether some credential family's prefix starts with `byte`.
    pub(super) fn may_start(byte: u8) -> bool {
        LEADING_BYTES
            .get(usize::from(byte.to_ascii_lowercase()))
            .is_some_and(|leads| *leads)
    }

    /// The end of this credential when it starts at `index`.
    pub(super) fn end_at(&self, bytes: &[u8], index: usize) -> Option<usize> {
        let prefix = self.prefix.as_bytes();
        let body = index + prefix.len();
        if !bytes.get(index..body)?.eq_ignore_ascii_case(prefix) {
            return None;
        }
        let end = self.body.run(bytes, body);
        (end - body >= self.minimum).then_some(end)
    }
}

const ASCII: usize = 128;

const LEADING_BYTES: [bool; ASCII] = leading_bytes(CREDENTIALS);

const fn leading_bytes(credentials: &[Credential]) -> [bool; ASCII] {
    let mut leads = [false; ASCII];
    let mut index = 0;
    while index < credentials.len() {
        let lead = credentials[index].prefix.as_bytes()[0].to_ascii_lowercase();
        leads[lead as usize] = true;
        index += 1;
    }
    leads
}

pub(super) const CREDENTIALS: &[Credential] = &[
    Credential::new("npm_", CharacterSet::Token, 8),
    Credential::new("ghp_", CharacterSet::Token, 8),
    Credential::new("gho_", CharacterSet::Token, 8),
    Credential::new("ghu_", CharacterSet::Token, 8),
    Credential::new("ghs_", CharacterSet::Token, 8),
    Credential::new("ghr_", CharacterSet::Token, 8),
    Credential::new("github_pat_", CharacterSet::Token, 8),
    Credential::new("sk-", CharacterSet::Token, 8),
    Credential::new("xoxb-", CharacterSet::Token, 8),
    Credential::new("xoxa-", CharacterSet::Token, 8),
    Credential::new("xoxp-", CharacterSet::Token, 8),
    Credential::new("xoxr-", CharacterSet::Token, 8),
    Credential::new("xoxs-", CharacterSet::Token, 8),
    Credential::new("xapp-", CharacterSet::Token, 8),
    Credential::new("lin_api_", CharacterSet::Token, 8),
    Credential::new("sntryu_", CharacterSet::Token, 8),
    Credential::new("sntrys_", CharacterSet::Token, 8),
    Credential::new("glpat-", CharacterSet::Word, 16),
    Credential::new("sk_live_", CharacterSet::Encoded, 16),
    Credential::new("sk_test_", CharacterSet::Encoded, 16),
    Credential::new("rk_live_", CharacterSet::Encoded, 16),
    Credential::new("rk_test_", CharacterSet::Encoded, 16),
    Credential::new("hf_", CharacterSet::Alphanumeric, 30),
    Credential::new("AIzaSy", CharacterSet::Word, 20),
];
