use aes_gcm::Aes256Gcm;
use aes_gcm::Nonce;
use aes_gcm::aead::Aead;
use aes_gcm::aead::KeyInit;
use aes_gcm::aead::consts::U12;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use zeroize::Zeroize;

use crate::error::SecretError;
use crate::key::MasterKey;
use crate::random;
use crate::value::SecretValue;

const ENVELOPE_PREFIX: &str = "ENC[v1:";
const ENVELOPE_SUFFIX: &str = "]";
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;

/// Whether a stored value is already wrapped in an envelope.
pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(ENVELOPE_PREFIX) && value.ends_with(ENVELOPE_SUFFIX)
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
/// # Ok::<(), abnegate_secret::SecretError>(())
/// ```
pub fn encrypt_value(value: &SecretValue, key: &MasterKey) -> Result<String, SecretError> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| SecretError::Encryption)?;

    let mut nonce_bytes = [0u8; NONCE_BYTES];
    random::fill(&mut nonce_bytes)?;
    let nonce =
        Nonce::<U12>::try_from(nonce_bytes.as_slice()).map_err(|_| SecretError::Encryption)?;

    let ciphertext = cipher
        .encrypt(&nonce, value.expose().as_bytes())
        .map_err(|_| SecretError::Encryption)?;

    let mut sealed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
    sealed.extend_from_slice(&nonce_bytes);
    sealed.extend_from_slice(&ciphertext);

    Ok(format!(
        "{ENVELOPE_PREFIX}{}{ENVELOPE_SUFFIX}",
        BASE64.encode(&sealed)
    ))
}

/// Unwrap a stored value, passing plaintext straight through.
///
/// Values written before the envelope existed keep working; each one is
/// reported through `tracing::debug!` as it is read. A value that opens an
/// envelope without closing it is [`SecretError::Truncated`], never plaintext.
pub fn decrypt_value(value: &str, key: &MasterKey) -> Result<SecretValue, SecretError> {
    let Some(body) = value.strip_prefix(ENVELOPE_PREFIX) else {
        tracing::debug!(
            "Plaintext secret read from storage; re-save it to seal it in an ENC[v1:...] envelope"
        );
        return Ok(SecretValue::new(value));
    };

    let encoded = body
        .strip_suffix(ENVELOPE_SUFFIX)
        .ok_or(SecretError::Truncated)?;
    let sealed = BASE64
        .decode(encoded)
        .map_err(|_| SecretError::InvalidBase64)?;

    if sealed.len() < NONCE_BYTES + TAG_BYTES {
        return Err(SecretError::Truncated);
    }

    let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_BYTES);
    let nonce = Nonce::<U12>::try_from(nonce_bytes).map_err(|_| SecretError::Decryption)?;

    let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| SecretError::Decryption)?;
    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| SecretError::Decryption)?;

    match String::from_utf8(plaintext) {
        Ok(text) => Ok(SecretValue::new(text)),
        Err(error) => {
            let mut bytes = error.into_bytes();
            bytes.zeroize();
            Err(SecretError::InvalidUtf8)
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
        assert!(sealed.starts_with(ENVELOPE_PREFIX));
        assert!(sealed.ends_with(ENVELOPE_SUFFIX));
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
            Err(SecretError::Decryption)
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
            Err(SecretError::InvalidBase64)
        ));
    }

    #[test]
    fn rejects_a_truncated_envelope() {
        let key = MasterKey::generate().unwrap();
        let short = format!(
            "{ENVELOPE_PREFIX}{}{ENVELOPE_SUFFIX}",
            BASE64.encode(b"short")
        );
        assert!(matches!(
            decrypt_value(&short, &key),
            Err(SecretError::Truncated)
        ));
    }

    #[test]
    fn a_value_that_opens_an_envelope_without_closing_it_is_not_plaintext() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new("secret"), &key).unwrap();

        for truncated in [
            &sealed[..sealed.len() - 1],
            &sealed[..sealed.len() / 2],
            ENVELOPE_PREFIX,
        ] {
            assert!(
                matches!(decrypt_value(truncated, &key), Err(SecretError::Truncated)),
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
    fn a_key_restored_from_hexadecimal_opens_the_same_envelope() {
        let key = MasterKey::generate().unwrap();
        let sealed = encrypt_value(&SecretValue::new("sk-live-restored"), &key).unwrap();

        let restored = MasterKey::from_hex(&key.to_hex()).unwrap();
        assert_eq!(
            decrypt_value(&sealed, &restored).unwrap().expose(),
            "sk-live-restored"
        );
    }
}
