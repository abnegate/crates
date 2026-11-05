use crate::redact::character_set::CharacterSet;
use crate::redact::evidence::Evidence;
use crate::redact::reads_as_words;

/// A credential family recognised by its prefix.
pub(super) struct Credential {
    prefix: &'static str,
    body: CharacterSet,
    minimum: usize,
    evidence: Evidence,
}

impl Credential {
    const fn new(
        prefix: &'static str,
        body: CharacterSet,
        minimum: usize,
        evidence: Evidence,
    ) -> Self {
        Self {
            prefix,
            body,
            minimum,
            evidence,
        }
    }

    /// Whether some credential family's prefix starts with `byte`.
    pub(super) fn may_start(byte: u8) -> bool {
        LEADING_BYTES
            .get(usize::from(byte.to_ascii_lowercase()))
            .is_some_and(|leads| *leads)
    }

    /// The end of this credential when it starts at `index`.
    ///
    /// A body that reads as words, as the `learn-experiments` of
    /// `~/sk-learn-experiments` does, is a name rather than a credential. It is
    /// kept in `prose`, which spares the credentials it [covers](Prose) a look
    /// inside it.
    pub(super) fn end_at(
        &self,
        bytes: &[u8],
        index: usize,
        prose: &mut Option<Prose>,
    ) -> Option<usize> {
        if prose.is_some_and(|prose| prose.covers(self, index)) {
            return None;
        }
        let body = self.body_at(bytes, index)?;
        let end = self.body.run(bytes, body);
        if end - body < self.minimum {
            return None;
        }
        if reads_as_words(&bytes[body..end]) {
            *prose = Some(Prose {
                end,
                stop: bytes.get(end).copied(),
            });
            return None;
        }
        Some(end)
    }

    /// Whether this credential starts at `index` of a value read whole.
    ///
    /// Only one whose [evidence](Evidence) is its prefix does. Unlike
    /// [`end_at`](Self::end_at), its body still counts when it reads as words,
    /// and a segment runs on through the path separators that end it in text:
    /// a value meant to be a plain name has no use for such a prefix at all.
    /// One whose evidence is its body is left to [`end_at`](Self::end_at).
    pub(super) fn starts_at(&self, bytes: &[u8], index: usize) -> bool {
        self.evidence == Evidence::Prefix
            && self.body_at(bytes, index).is_some_and(|body| {
                self.body.through_paths().run(bytes, body) - body >= self.minimum
            })
    }

    /// Where this credential's body starts when its prefix, in any case, is at
    /// `index`.
    fn body_at(&self, bytes: &[u8], index: usize) -> Option<usize> {
        let prefix = self.prefix.as_bytes();
        let body = index + prefix.len();
        bytes
            .get(index..body)?
            .eq_ignore_ascii_case(prefix)
            .then_some(body)
    }
}

/// A credential body that reads as words, and the byte that stopped it.
///
/// A credential whose prefix fits inside it, and whose body cannot hold that
/// byte, would run no further than its end and read as words too, so it is not
/// looked for. One whose body can hold the byte runs past it, and so no longer
/// reads as words: it is a credential, or too short to be one, and either way
/// the body is not read again.
#[derive(Clone, Copy)]
pub(super) struct Prose {
    end: usize,
    stop: Option<u8>,
}

impl Prose {
    fn covers(self, credential: &Credential, index: usize) -> bool {
        index + credential.prefix.len() <= self.end
            && self.stop.is_none_or(|byte| !credential.body.contains(byte))
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
    Credential::new("npm_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("ghp_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("gho_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("ghu_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("ghs_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("ghr_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("github_pat_", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("sk-", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("xai-", CharacterSet::Segment, 8, Evidence::Body),
    Credential::new("xai_", CharacterSet::Segment, 8, Evidence::Body),
    Credential::new("xoxb-", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("xoxa-", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("xoxp-", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("xoxr-", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("xoxs-", CharacterSet::Segment, 8, Evidence::Prefix),
    Credential::new("xapp-", CharacterSet::Segment, 8, Evidence::Body),
    Credential::new("lin_api_", CharacterSet::Segment, 8, Evidence::Body),
    Credential::new("sntryu_", CharacterSet::Segment, 8, Evidence::Body),
    Credential::new("sntrys_", CharacterSet::Token, 8, Evidence::Body),
    Credential::new("glpat-", CharacterSet::Word, 16, Evidence::Prefix),
    Credential::new("sk_live_", CharacterSet::Word, 16, Evidence::Prefix),
    Credential::new("sk_test_", CharacterSet::Word, 16, Evidence::Body),
    Credential::new("sk_", CharacterSet::Word, 16, Evidence::Body),
    Credential::new("rk_live_", CharacterSet::Word, 16, Evidence::Body),
    Credential::new("rk_test_", CharacterSet::Word, 16, Evidence::Body),
    Credential::new("hf_", CharacterSet::Alphanumeric, 30, Evidence::Body),
    Credential::new("AIzaSy", CharacterSet::Word, 20, Evidence::Prefix),
];

/// An AWS access key ID read as loosely as any other prefix, for a value read
/// whole. Redaction matches the exact twenty-character shape instead, so that
/// it takes out the key and nothing after it.
pub(super) const AWS_ACCESS_KEY: Credential =
    Credential::new("AKIA", CharacterSet::Token, 8, Evidence::Prefix);
