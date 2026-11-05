//! The bare format sealed before the envelope existed, held to a value already
//! sitting in storage, and plaintext held to never passing for it.

#![cfg(feature = "encryption")]

use abnegate_secret::Error;
use abnegate_secret::MasterKey;
use abnegate_secret::decrypt_legacy_value;
use abnegate_secret::decrypt_value;
use abnegate_secret::is_encrypted;

/// The key [`LEGACY`] is sealed under: the bytes 0 to 31.
const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// [`PLAINTEXT`] sealed under [`KEY`] by the format this crate absorbed, before
/// the envelope existed. Never regenerate it: it stands for every bare value
/// already in storage.
const LEGACY: &str =
    "HUbaNoY9b1ZdzgAd/79AdG3rW0wAfUoHYkCqlVqoDKB/z7P/i+plujqrIVdsl24Pl9Lm1nnLfyV2cNp9C6KO6JM=";

/// The credential sealed in [`LEGACY`].
const PLAINTEXT: &str = "sk-live-legacy-known-answer-clé-🔐";

fn key() -> MasterKey {
    MasterKey::from_hexadecimal(KEY).unwrap()
}

#[test]
fn a_value_sealed_before_the_envelope_existed_still_opens() {
    assert!(!is_encrypted(LEGACY));
    assert_eq!(
        decrypt_legacy_value(LEGACY, &key()).unwrap().expose(),
        PLAINTEXT
    );
}

#[test]
fn the_wrong_key_is_refused() {
    let opened = decrypt_legacy_value(LEGACY, &MasterKey::generate().unwrap());
    assert!(matches!(opened, Err(Error::Decryption)), "{opened:?}");
}

#[test]
fn plaintext_is_refused() {
    let opened = decrypt_legacy_value("ghp_plaintext_token!", &key());
    assert!(matches!(opened, Err(Error::InvalidBase64)), "{opened:?}");
}

#[test]
fn plaintext_that_is_short_base64_is_refused() {
    let opened = decrypt_legacy_value("AAAAAAAAAAAAAAAAAAAAAAAAAAAA", &key());
    assert!(matches!(opened, Err(Error::Truncated)), "{opened:?}");
}

#[test]
fn an_empty_value_is_refused() {
    let opened = decrypt_legacy_value("", &key());
    assert!(matches!(opened, Err(Error::Truncated)), "{opened:?}");
}

#[test]
fn a_hexadecimal_token_that_is_valid_base64_is_refused() {
    let opened = decrypt_legacy_value("0123456789abcdef0123456789abcdef01234567", &key());
    assert!(matches!(opened, Err(Error::Decryption)), "{opened:?}");
}

#[test]
fn decrypt_value_still_hands_a_bare_body_back_as_text() {
    assert_eq!(decrypt_value(LEGACY, &key()).unwrap().expose(), LEGACY);
}
