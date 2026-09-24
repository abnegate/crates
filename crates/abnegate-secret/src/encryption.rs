mod envelope;

use aes_gcm::Aes256Gcm;
use aes_gcm::Nonce;
use aes_gcm::aead::Aead;
use aes_gcm::aead::KeyInit;
use aes_gcm::aead::consts::U12;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use zeroize::Zeroize;

use crate::encryption::envelope::Envelope;
use crate::error::Error;
use crate::key::MasterKey;
use crate::random;
use crate::value::SecretValue;

const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

/// Whether a stored value is already wrapped in an envelope: `ENC[v`, a
/// version number, `:`, and the sealed value up to a closing `]`.
///
/// An envelope of any version counts, including one this release cannot
/// open, so a value sealed by a later release is never taken for plaintext.
/// [`decrypt_value`] refuses those with [`Error::UnsupportedVersion`].
pub fn is_encrypted(value: &str) -> bool {
    Envelope::open(value).is_some_and(|envelope| envelope.encoded().is_some())
}

/// Wrap a credential in an envelope sealed with `key`, under a nonce drawn
/// from the operating system's random source.
///
/// ```
/// use abnegate_secret::{MasterKey, SecretValue, decrypt_value, encrypt_value};
///
/// let key = MasterKey::generate()?;
/// let sealed = encrypt_value(&SecretValue::new("sk-live-0123456789"), &key)?;
/// assert!(sealed.starts_with("ENC[v1:"));
/// assert_eq!(decrypt_value(&sealed, &key)?.expose(), "sk-live-0123456789");
/// # Ok::<(), abnegate_secret::Error>(())
/// ```
pub fn encrypt_value(value: &SecretValue, key: &MasterKey) -> Result<String, Error> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| Error::Encryption)?;

    let mut nonce_bytes = [0u8; NONCE_BYTES];
    random::fill(&mut nonce_bytes)?;
    let nonce = Nonce::<U12>::try_from(nonce_bytes.as_slice()).map_err(|_| Error::Encryption)?;

    let ciphertext = cipher
        .encrypt(&nonce, value.expose().as_bytes())
        .map_err(|_| Error::Encryption)?;

    let mut sealed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
    sealed.extend_from_slice(&nonce_bytes);
    sealed.extend_from_slice(&ciphertext);

    Ok(Envelope::seal(&BASE64.encode(&sealed)))
}

/// Unwrap a stored value, passing plaintext straight through.
///
/// Values written before the envelope existed keep working; each one is
/// reported through `tracing::debug!` as it is read. A value that opens an
/// envelope is never plaintext: an envelope of any version but v1 is
/// [`Error::UnsupportedVersion`], and one that is never closed is
/// [`Error::Truncated`].
pub fn decrypt_value(value: &str, key: &MasterKey) -> Result<SecretValue, Error> {
    let Some(envelope) = Envelope::open(value) else {
        tracing::debug!(
            "Plaintext secret read from storage; re-save it to seal it in an ENC[v1:...] envelope"
        );
        return Ok(SecretValue::new(value));
    };

    if envelope.version() != Envelope::VERSION {
        return Err(Error::UnsupportedVersion {
            version: envelope.version().to_owned(),
        });
    }

    let encoded = envelope.encoded().ok_or(Error::Truncated)?;
    let sealed = BASE64.decode(encoded).map_err(|_| Error::InvalidBase64)?;

    if sealed.len() < NONCE_BYTES + TAG_BYTES {
        return Err(Error::Truncated);
    }

    let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_BYTES);
    let nonce = Nonce::<U12>::try_from(nonce_bytes).map_err(|_| Error::Decryption)?;

    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| Error::Decryption)?;
    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| Error::Decryption)?;

    match String::from_utf8(plaintext) {
        Ok(text) => Ok(SecretValue::new(text)),
        Err(error) => {
            let mut bytes = error.into_bytes();
            bytes.zeroize();
            Err(Error::InvalidUtf8)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use tracing::Event;
    use tracing::Level;
    use tracing::Metadata;
    use tracing::Subscriber;
    use tracing::span;

    use super::*;

    struct LevelRecorder {
        levels: Arc<Mutex<Vec<Level>>>,
    }

    impl Subscriber for LevelRecorder {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}

        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

        fn event(&self, event: &Event<'_>) {
            self.levels.lock().unwrap().push(*event.metadata().level());
        }

        fn enter(&self, _: &span::Id) {}

        fn exit(&self, _: &span::Id) {}
    }

    #[test]
    fn round_trips_a_credential() {
        let key = MasterKey::generate().unwrap();
        let secret = SecretValue::new("sk-live-0123456789abcdef");

        let sealed = encrypt_value(&secret, &key).unwrap();
        assert!(is_encrypted(&sealed));
        assert!(sealed.starts_with("ENC[v1:"));
        assert!(sealed.ends_with(']'));
        assert!(!sealed.contains(secret.expose()));

        assert_eq!(decrypt_value(&sealed, &key).unwrap(), secret);
    }

    #[test]
    fn round_trips_an_empty_value() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new(""), &key).unwrap();
        assert!(decrypt_value(&sealed, &key).unwrap().is_empty());
    }

    #[test]
    fn round_trips_multibyte_text() {
        let key = MasterKey::generate().unwrap();
        let secret = SecretValue::new("clé-secrète-🔐-中文");
        let sealed = encrypt_value(&secret, &key).unwrap();
        assert_eq!(decrypt_value(&sealed, &key).unwrap(), secret);
    }

    #[test]
    fn passes_plaintext_through() {
        let key = MasterKey::generate().unwrap();
        let plaintext = "written-before-the-envelope-existed";
        assert_eq!(decrypt_value(plaintext, &key).unwrap().expose(), plaintext);
    }

    #[test]
    fn every_envelope_uses_a_fresh_nonce() {
        let key = MasterKey::generate().unwrap();
        let secret = SecretValue::new("same-secret");

        let first = encrypt_value(&secret, &key).unwrap();
        let second = encrypt_value(&secret, &key).unwrap();

        assert_ne!(first, second);
        assert_eq!(decrypt_value(&first, &key).unwrap(), secret);
        assert_eq!(decrypt_value(&second, &key).unwrap(), secret);
    }

    #[test]
    fn rejects_the_wrong_key() {
        let sealed =
            encrypt_value(&SecretValue::new("secret"), &MasterKey::generate().unwrap()).unwrap();
        assert!(matches!(
            decrypt_value(&sealed, &MasterKey::generate().unwrap()),
            Err(Error::Decryption)
        ));
    }

    #[test]
    fn rejects_a_tampered_envelope() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new("secret"), &key).unwrap();
        let mut tampered = sealed.into_bytes();
        let last = tampered.len() - 2;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };

        let tampered = String::from_utf8(tampered).unwrap();
        assert!(decrypt_value(&tampered, &key).is_err());
    }

    #[test]
    fn rejects_invalid_base64() {
        let key = MasterKey::generate().unwrap();
        assert!(matches!(
            decrypt_value("ENC[v1:not!valid@base64#]", &key),
            Err(Error::InvalidBase64)
        ));
    }

    #[test]
    fn rejects_a_truncated_envelope() {
        let key = MasterKey::generate().unwrap();
        let short = Envelope::seal(&BASE64.encode(b"short"));
        assert!(matches!(decrypt_value(&short, &key), Err(Error::Truncated)));
    }

    #[test]
    fn a_value_that_opens_an_envelope_without_closing_it_is_not_plaintext() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new("secret"), &key).unwrap();

        for truncated in [
            &sealed[..sealed.len() - 1],
            &sealed[..sealed.len() / 2],
            "ENC[v1:",
        ] {
            assert!(
                matches!(decrypt_value(truncated, &key), Err(Error::Truncated)),
                "{truncated} was read as plaintext"
            );
        }
    }

    #[test]
    fn reading_plaintext_is_reported_below_warning_level() {
        let levels = Arc::new(Mutex::new(Vec::new()));
        let recorder = LevelRecorder {
            levels: Arc::clone(&levels),
        };
        let key = MasterKey::generate().unwrap();

        tracing::subscriber::with_default(recorder, || {
            for _ in 0..3 {
                decrypt_value("written-before-the-envelope-existed", &key).unwrap();
            }
        });

        let levels = levels.lock().unwrap();
        assert_eq!(levels.as_slice(), [Level::DEBUG; 3]);
    }

    #[test]
    fn recognises_the_envelope_format() {
        assert!(is_encrypted("ENC[v1:YWJj]"));
        assert!(!is_encrypted("plaintext"));
        assert!(!is_encrypted("ENC[v1:missing-suffix"));
        assert!(!is_encrypted("WRONG[v1:YWJj]"));
    }

    #[test]
    fn recognises_an_envelope_of_any_version() {
        for sealed in [
            "ENC[v1:YWJj]",
            "ENC[v2:YWJj]",
            "ENC[v10:YWJj]",
            "ENC[v01:YWJj]",
            "ENC[v18446744073709551616:YWJj]",
        ] {
            assert!(is_encrypted(sealed), "{sealed} was taken for plaintext");
        }
        assert!(!is_encrypted("ENC[v2:missing-suffix"));
    }

    #[test]
    fn a_version_that_is_not_a_number_opens_no_envelope() {
        let key = MasterKey::generate().unwrap();

        for plaintext in [
            "ENC[v:YWJj]",
            "ENC[vx:YWJj]",
            "ENC[v1x:YWJj]",
            "ENC[v-1:YWJj]",
            "ENC[v1YWJj]",
            "enc[v1:YWJj]",
        ] {
            assert!(!is_encrypted(plaintext), "{plaintext}");
            assert_eq!(
                decrypt_value(plaintext, &key).unwrap().expose(),
                plaintext,
                "{plaintext}"
            );
        }
    }

    #[test]
    fn every_version_but_v1_is_refused_whatever_it_holds() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new("secret"), &key).unwrap();
        let body = sealed.strip_prefix("ENC[v1:").unwrap();

        for (expected, value) in [
            ("2", format!("ENC[v2:{body}")),
            ("0", format!("ENC[v0:{body}")),
            ("01", format!("ENC[v01:{body}")),
            (
                "18446744073709551616",
                format!("ENC[v18446744073709551616:{body}"),
            ),
            ("2", "ENC[v2:not-closed".to_owned()),
            ("2", "ENC[v2:".to_owned()),
        ] {
            let opened = decrypt_value(&value, &key);
            assert!(
                matches!(&opened, Err(Error::UnsupportedVersion { version }) if version == expected),
                "{value}: {opened:?}"
            );
        }
    }

    #[test]
    fn a_key_restored_from_hexadecimal_opens_the_same_envelope() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new("sk-live-restored"), &key).unwrap();

        let restored = MasterKey::from_hexadecimal(&key.to_hexadecimal()).unwrap();
        assert_eq!(
            decrypt_value(&sealed, &restored).unwrap().expose(),
            "sk-live-restored"
        );
    }
}
