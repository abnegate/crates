//! The stored envelope format, held to values already sitting in storage.

use abnegate_secret::MasterKey;
use abnegate_secret::decrypt_value;
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

#[test]
fn a_v1_envelope_already_in_storage_still_opens() {
    let key = MasterKey::from_hexadecimal(KEY).unwrap();

    assert!(is_encrypted(ENVELOPE));
    assert_eq!(decrypt_value(ENVELOPE, &key).unwrap().expose(), PLAINTEXT);
}
