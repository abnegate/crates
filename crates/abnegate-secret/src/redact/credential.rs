use crate::redact::character_set::CharacterSet;
use crate::redact::reads_as_words;

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
        let prefix = self.prefix.as_bytes();
        let body = index + prefix.len();
        if !bytes.get(index..body)?.eq_ignore_ascii_case(prefix) {
            return None;
        }
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
    Credential::new("npm_", CharacterSet::Segment, 8),
    Credential::new("ghp_", CharacterSet::Segment, 8),
    Credential::new("gho_", CharacterSet::Segment, 8),
    Credential::new("ghu_", CharacterSet::Segment, 8),
    Credential::new("ghs_", CharacterSet::Segment, 8),
    Credential::new("ghr_", CharacterSet::Segment, 8),
    Credential::new("github_pat_", CharacterSet::Segment, 8),
    Credential::new("sk-", CharacterSet::Segment, 8),
    Credential::new("xai-", CharacterSet::Segment, 8),
    Credential::new("xai_", CharacterSet::Segment, 8),
    Credential::new("xoxb-", CharacterSet::Segment, 8),
    Credential::new("xoxa-", CharacterSet::Segment, 8),
    Credential::new("xoxp-", CharacterSet::Segment, 8),
    Credential::new("xoxr-", CharacterSet::Segment, 8),
    Credential::new("xoxs-", CharacterSet::Segment, 8),
    Credential::new("xapp-", CharacterSet::Segment, 8),
    Credential::new("lin_api_", CharacterSet::Segment, 8),
    Credential::new("sntryu_", CharacterSet::Segment, 8),
    Credential::new("sntrys_", CharacterSet::Token, 8),
    Credential::new("glpat-", CharacterSet::Word, 16),
    Credential::new("sk_live_", CharacterSet::Word, 16),
    Credential::new("sk_test_", CharacterSet::Word, 16),
    Credential::new("sk_", CharacterSet::Word, 16),
    Credential::new("rk_live_", CharacterSet::Word, 16),
    Credential::new("rk_test_", CharacterSet::Word, 16),
    Credential::new("hf_", CharacterSet::Alphanumeric, 30),
    Credential::new("AIzaSy", CharacterSet::Word, 20),
];
