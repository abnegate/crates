use crate::redact::MINIMUM_ENCODED_LENGTH;
use crate::redact::carries_shaped_secret;
use crate::redact::character_set::CharacterSet;
use crate::redact::credential::AWS_ACCESS_KEY;
use crate::redact::credential::CREDENTIALS;
use crate::redact::is_high_entropy;
use crate::work;

/// `ſ`, which Unicode case folding equates with `s`.
const LONG_S: &[u8] = "\u{17F}".as_bytes();
/// The Kelvin sign, which Unicode case folding equates with `k`.
const KELVIN: &[u8] = "\u{212A}".as_bytes();

/// Whether `value` carries a credential anywhere in it.
///
/// Meant for a value that should be a plain name, such as a script, a tool or
/// a search term, and that is refused rather than echoed into a citation or a
/// log when it looks like a credential. It is true wherever
/// [`redact`](crate::redact) would remove something, but for a value assigned
/// to a key word that carries no digit and is shorter than 16 bytes or reads
/// as words, as the `refresh` of a script named `token:refresh` does.
///
/// It errs toward true beyond that. A widely issued prefix, such as `ghp_`,
/// `sk-` or `AKIA`, with enough after it counts even when what follows reads
/// as words, or when it is spelt with `ſ` for `s` or the Kelvin sign for `k`,
/// and so does any run of 32 or more base64 or URL-safe characters that mixes
/// letters with digits at high entropy, a commit hash included, whether or not
/// an assignment names it. A less widely issued prefix, such as `xai_` or
/// `sk_`, counts only where redaction would remove it, so that
/// `xai_list_models` is a name.
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
    let bytes = value.as_bytes();
    issued_or_high_entropy(bytes)
        || (has_folding(bytes) && issued(&folded(bytes)))
        || carries_shaped_secret(value)
}

fn issued_or_high_entropy(bytes: &[u8]) -> bool {
    work::find(bytes, 0, |index| {
        issued_at(bytes, index) || high_entropy_at(bytes, index)
    })
    .is_some()
}

fn issued(bytes: &[u8]) -> bool {
    work::find(bytes, 0, |index| issued_at(bytes, index)).is_some()
}

fn has_folding(bytes: &[u8]) -> bool {
    work::find(bytes, 0, |index| {
        bytes[index..].starts_with(LONG_S) || bytes[index..].starts_with(KELVIN)
    })
    .is_some()
}

/// `bytes` with each [`LONG_S`] and [`KELVIN`] folded to the ASCII letter it
/// equates with.
fn folded(bytes: &[u8]) -> Vec<u8> {
    let mut folded = Vec::new();
    work::each_index(bytes, |index| {
        if let Some(byte) = folded_at(bytes, index) {
            folded.push(byte);
        }
    });
    folded
}

/// What the byte at `index` folds to: itself, the letter that a folding
/// character it ends equates with, or nothing when it starts one.
fn folded_at(bytes: &[u8], index: usize) -> Option<u8> {
    let through = |end: usize| bytes.get(..end).unwrap_or(&[]);
    if through(index + 1).ends_with(LONG_S) {
        Some(b's')
    } else if through(index + 1).ends_with(KELVIN) {
        Some(b'k')
    } else if through(index + LONG_S.len()).ends_with(LONG_S)
        || through(index + KELVIN.len() - 1).ends_with(KELVIN)
        || through(index + KELVIN.len()).ends_with(KELVIN)
    {
        None
    } else {
        Some(bytes[index])
    }
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
    fn a_name_namespaced_by_a_key_word_is_not_a_credential() {
        for value in [
            "token:refresh",
            "token:generate",
            "tokens:revoke",
            "secrets:rotate",
            "access_token:refresh",
            "api_key:rotate",
            "signature:verify",
            "token:refresh-every-session",
            "Remember to rotate the secret: weekly",
        ] {
            assert!(!contains_credential(value), "{value}");
            assert_ne!(crate::redact(value), value, "redaction still takes it out");
        }
    }

    #[test]
    fn a_name_spelt_like_a_less_known_credential_is_not_one() {
        for value in [
            "xai_list_models",
            "xai-chat-completions",
            "xapp-socket-mode",
            "lin_api_create_issue",
            "sntryu_release_health",
            "sk_test_connection_pool",
            "rk_live_reload_watcher",
            "tools/testing/selftests/net/sk_bind_sendto_listen.c",
            "search sk_write_pending_queue",
        ] {
            assert!(!contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_value_a_key_word_names_counts_when_shaped_like_a_credential() {
        for value in [
            "token:refresh2",
            "password:P4ssword",
            "secret: a1b2c3d4",
            "token:Xq7Lm/Zp+Rt",
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn a_prefix_spelt_with_a_folding_character_counts() {
        for value in [
            concat!("\u{17F}k-", "a1b2c3d4"),
            concat!("s\u{212A}-", "a1b2c3d4"),
            concat!("A\u{212A}IA", "a1b2c3d4"),
            concat!("xoxa-", "Zk\u{17F}q8Lr2Vx7"),
            concat!("ghp_", "a1b\u{212A}2c3d"),
            concat!("Remember my key is \u{17F}k-", "proj1234abcd"),
        ] {
            assert!(contains_credential(value), "{value}");
        }
    }

    #[test]
    fn other_characters_outside_ascii_do_not_lengthen_a_body() {
        for value in [
            "npm_install\u{2014}fa\u{17F}t",
            "\u{17F}k-abc\u{142}defgh",
            "\u{17F}k-gro\u{DF}stadt",
            "s\u{212A}-\u{1F600}abcdefgh",
        ] {
            assert!(!contains_credential(value), "{value}");
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
        for unit in ["\u{17F}k-a1 ", "x\u{212A}\u{17F}", "\u{17F}"] {
            work::assert_linear(
                LENGTH / unit.len(),
                |repetitions| unit.repeat(repetitions),
                |value| {
                    let bytes = value.as_bytes();
                    assert!(!(has_folding(bytes) && issued(&folded(bytes))), "{value}");
                },
            );
        }
        for unit in [
            "token:refresh ",
            "secret:abcdefghijklmno ",
            "token:refresh-every-session ",
            "token=\"refresh\" ",
        ] {
            work::assert_linear(
                LENGTH / unit.len(),
                |repetitions| unit.repeat(repetitions),
                |value| {
                    assert!(!carries_shaped_secret(value), "{value}");
                },
            );
        }
    }
}
