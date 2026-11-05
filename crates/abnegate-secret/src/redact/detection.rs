use std::borrow::Cow;

use crate::redact::MINIMUM_ENCODED_LENGTH;
use crate::redact::character_set::CharacterSet;
use crate::redact::credential::AWS_ACCESS_KEY;
use crate::redact::credential::CREDENTIALS;
use crate::redact::is_high_entropy;
use crate::redact::redact;
use crate::work;

/// Whether `value` carries a credential anywhere in it.
///
/// Meant for a value that should be a plain name, such as a script, a tool or
/// a search term, and that is refused rather than echoed into a citation or a
/// log when it looks like a credential. It is true wherever [`redact`] would
/// remove something, and errs toward true beyond that: a credential prefix
/// with enough after it counts even when what follows reads as words, and so
/// does any run of 32 or more base64 or URL-safe characters that mixes letters
/// with digits at high entropy, a commit hash included, whether or not an
/// assignment names it.
///
/// Time is linear in the length of `value`.
///
/// ```
/// use abnegate_secret::contains_credential;
///
/// assert!(contains_credential(concat!("ghp_", "0123456789abcdefghij")));
/// assert!(!contains_credential("cargo-test-all"));
/// ```
pub fn contains_credential(value: &str) -> bool {
    issued_or_high_entropy(value.as_bytes()) || matches!(redact(value), Cow::Owned(_))
}

fn issued_or_high_entropy(bytes: &[u8]) -> bool {
    work::find(bytes, 0, |index| {
        issued_at(bytes, index) || high_entropy_at(bytes, index)
    })
    .is_some()
}

fn issued_at(bytes: &[u8], index: usize) -> bool {
    (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
        && CREDENTIALS
            .iter()
            .chain([&AWS_ACCESS_KEY])
            .any(|credential| credential.starts_at(bytes, index))
}

fn high_entropy_at(bytes: &[u8], index: usize) -> bool {
    if index > 0 && CharacterSet::Encoded.contains(bytes[index - 1]) {
        return false;
    }
    let end = CharacterSet::Encoded.run(bytes, index);
    end - index >= MINIMUM_ENCODED_LENGTH && is_high_entropy(&bytes[index..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LENGTH: usize = 4096;

    #[test]
    fn finds_each_issued_prefix() {
        for value in [
            concat!("npm_", "a1b2c3d4"),
            concat!("ghp_", "a1b2c3d4"),
            concat!("gho_", "a1b2c3d4"),
            concat!("ghu_", "a1b2c3d4"),
            concat!("ghs_", "a1b2c3d4"),
            concat!("ghr_", "a1b2c3d4"),
            concat!("github_pat_", "a1b2c3d4"),
            concat!("sk-", "a1b2c3d4"),
            concat!("xoxb-", "a1b2c3d4"),
            concat!("xoxa-", "a1b2c3d4"),
            concat!("xoxp-", "a1b2c3d4"),
            concat!("xoxr-", "a1b2c3d4"),
            concat!("xoxs-", "a1b2c3d4"),
            concat!("AKIA", "a1b2c3d4"),
            concat!("glpat-", "a1b2c3d4e5f6g7h8"),
            concat!("sk_live_", "a1b2c3d4e5f6g7h8"),
            concat!("AIzaSy", "a1b2c3d4e5f6g7h8i9j0"),
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn finds_the_other_prefixes_redaction_knows() {
        for value in [
            concat!("xai-", "a1b2c3d4"),
            concat!("lin_api_", "a1b2c3d4"),
            concat!("sntrys_", "a1b2c3d4"),
            concat!("hf_", "a1b2c3d4e5f6g7h8i9j0a1b2c3d4e5"),
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_prefix_needs_its_whole_minimum_body() {
        for value in [
            concat!("ghp_", "a1b2c3d"),
            concat!("AKIA", "a1b2c3d"),
            concat!("glpat-", "a1b2c3d4e5f6g7h"),
            concat!("sk_live_", "a1b2c3d"),
            concat!("AIzaSy", "a1b2c3d4e5f6g7h8i9j"),
        ] {
            assert!(!contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_prefix_matches_in_any_case_after_a_boundary() {
        for value in [
            concat!("GHP_", "a1b2c3d4"),
            concat!("akia", "a1b2c3d4"),
            concat!("aizasy", "a1b2c3d4e5f6g7h8i9j0"),
            concat!("run --token ghp_", "a1b2c3d4"),
            concat!("scripts/sk-", "a1b2c3d4"),
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_prefix_inside_a_word_is_not_one() {
        for value in [
            concat!("xghp_", "a1b2c3d4"),
            concat!("task-", "a1b2c3d4"),
            concat!("BAKIA", "a1b2c3d4"),
        ] {
            assert!(!contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_body_runs_on_through_path_separators() {
        assert!(contains_credential(concat!("npm_", "a1/b2c3d4")));
    }

    #[test]
    fn a_body_that_reads_as_words_still_counts() {
        for value in ["sk-learn-experiments", "npm_config_cache"] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn finds_a_json_web_token() {
        let token = concat!(
            "eyJhbGciOiJIUzI1NiJ9",
            ".eyJzdWIiOiIxMjM0NTY3ODkwIn0",
            ".dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
        );
        assert!(contains_credential(token));
        assert!(contains_credential(&format!("search {token} now")));
    }

    #[test]
    fn finds_a_high_entropy_run_with_no_prefix() {
        for value in [
            concat!("Zm9vYmFyYmF6cXV4", "MTIzNDU2Nzg5MGFiY2RlZg"),
            concat!("abcdefghijklmnop", "qrstuvwxyzabcdef0"),
            concat!("ec23e0f2b8b9e7c9", "770d508511a24a4bd4f873c8"),
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_high_entropy_run_needs_thirty_two_characters() {
        assert!(!contains_credential(concat!(
            "q8Lr2Vx7Tn4Kp1Wz",
            "9Ys3Jd6Hf0Gb5Mc"
        )));
    }

    #[test]
    fn leaves_a_plain_name_alone() {
        for value in [
            "cargo-test-all",
            "run_integration_suite",
            "src/components/ChatsPage.tsx",
            "internationalization-localization-configuration",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1",
            "/Users/dev/Local/project/runner/server/src/main.rs",
            "",
        ] {
            assert!(!contains_credential(value), "{value}");
        }
    }

    #[test]
    fn finds_whatever_redact_removes() {
        for value in [
            "login failed: password=hunter2",
            concat!("ASIA", "0123456789ABCDEF"),
            "https://user:p4ssw0rd@example.com/repository.git",
            concat!(
                "-----BEGIN PRIVATE KEY-----\nMIIEvQ\n",
                "-----END PRIVATE KEY-----"
            ),
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn reading_a_value_whole_is_linear_in_its_length() {
        for unit in [
            "ghp_a ",
            "sk_a1.",
            "a1b2c3d ",
            "hf_abcdefghijklmnopqrstuvwxyzabc.",
            "glpat-abcdefghijklmno.",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1 ",
        ] {
            work::assert_linear(
                LENGTH / unit.len(),
                |repetitions| unit.repeat(repetitions),
                |value| {
                    assert!(!issued_or_high_entropy(value.as_bytes()), "{value}");
                },
            );
        }
    }
}
