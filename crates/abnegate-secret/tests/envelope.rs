//! The stored envelope format, held to values already sitting in storage.

use abnegate_secret::Error;
use abnegate_secret::MasterKey;
use abnegate_secret::SecretValue;
use abnegate_secret::decrypt_value;
use abnegate_secret::encrypt_value;
use abnegate_secret::is_encrypted;

/// The key [`ENVELOPE`] is sealed under: the bytes 0 to 31.
const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// [`PLAINTEXT`] as `encrypt_value` sealed it under [`KEY`] before any other
/// envelope version existed. Never regenerate it: it stands for every v1 value
/// already in storage.
const ENVELOPE: &str =
    "ENC[v1:twJaKzfAsReH97VVziypNBcxlp6btJvz3pFWGK34o1ae9NkYoHLxHp1S+0/yMxHLZqrKOjTriZI8tQ==]";

/// The credential sealed in [`ENVELOPE`].
const PLAINTEXT: &str = "sk-live-known-answer-clé-🔐";

fn key() -> MasterKey {
    MasterKey::from_hexadecimal(KEY).unwrap()
}

#[test]
fn a_v1_envelope_already_in_storage_still_opens() {
    assert!(is_encrypted(ENVELOPE));
    assert_eq!(decrypt_value(ENVELOPE, &key()).unwrap().expose(), PLAINTEXT);
}

#[test]
fn a_value_sealed_today_is_a_v1_envelope_that_opens() {
    let sealed = encrypt_value(&SecretValue::new(PLAINTEXT), &key()).unwrap();

    assert!(sealed.starts_with("ENC[v1:"), "{sealed}");
    assert!(is_encrypted(&sealed));
    assert_eq!(decrypt_value(&sealed, &key()).unwrap().expose(), PLAINTEXT);
}

#[test]
fn an_envelope_of_a_later_version_is_recognised_as_sealed_and_refused() {
    let later = ENVELOPE.replacen("ENC[v1:", "ENC[v2:", 1);

    assert!(
        is_encrypted(&later),
        "{later} would be taken for plaintext and stored again unsealed"
    );
    let opened = decrypt_value(&later, &key());
    assert!(
        matches!(&opened, Err(Error::UnsupportedVersion { version, .. }) if version == "2"),
        "{opened:?}"
    );
}
