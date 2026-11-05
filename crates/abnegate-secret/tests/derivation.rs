//! Passphrase derivation, held to the bytes the absorbed derivation produces.
//! The known-answer tests run in every build. Argon2id is memory-hard, so the
//! ignored tests, which derive several keys each, run in a release build, as
//! CI does:
//!
//! ```sh
//! cargo test -p abnegate-secret --release --features derivation -- --ignored
//! ```

#![cfg(feature = "derivation")]

use abnegate_secret::Error;
use abnegate_secret::KeyDerivation;
use abnegate_secret::MasterKey;
use abnegate_secret::SecretValue;
use abnegate_secret::decrypt_value;
use abnegate_secret::encrypt_value;

/// The passphrase [`DERIVED`] is derived from.
const PASSPHRASE: &str = "a-high-entropy-passphrase-of-at-least-32-bytes";

/// The passphrase [`DERIVED_FROM_EXACT_BYTES`] is derived from: non-ASCII
/// characters and a trailing newline, which no trim or normalisation may touch.
const UNNORMALISED_PASSPHRASE: &str = "pässphrase-with-ünïcödé-and-a-trailing-newline\n";

/// The domain [`DERIVED`] and [`DERIVED_FROM_EXACT_BYTES`] are derived under.
const DOMAIN: &[u8] = b"abnegate-secret-derivation-v1";

/// [`PASSPHRASE`] stretched under [`DOMAIN`]. Computed by the absorbed
/// derivation, run verbatim with only its domain replaced, before this crate's
/// derivation existed; never regenerate it: it stands for every key already
/// derived from a passphrase.
const DERIVED: &str = "d1e5afd2f9eaad526e02dcfe601c47c4eebf5dc561eb72ba16aef3d05228d52f";

/// [`UNNORMALISED_PASSPHRASE`] stretched under [`DOMAIN`]. Computed before
/// this test existed by two independent oracles that agree: the absorbed
/// derivation, run verbatim with only its domain replaced, and the Argon2
/// reference command-line tool given the same salt recipe and the explicit
/// Argon2id parameters. Never regenerate it: it pins that a key is derived
/// from the passphrase's exact bytes.
const DERIVED_FROM_EXACT_BYTES: &str =
    "50de995ce40631e1324241f97753cea5cd5a37e012542c7262263c534a4dc9eb";

fn derive(passphrase: &str, domain: &[u8]) -> Result<MasterKey, Error> {
    MasterKey::derive(&SecretValue::new(passphrase), &KeyDerivation::new(domain))
}

#[test]
fn a_passphrase_derives_the_pinned_key() -> Result<(), Error> {
    assert_eq!(
        derive(PASSPHRASE, DOMAIN)?.to_hexadecimal().as_str(),
        DERIVED
    );
    Ok(())
}

#[test]
#[ignore = "runs Argon2id, so run it in a release build: cargo test -p abnegate-secret --release --features derivation -- --ignored"]
fn derivation_is_deterministic_and_bound_to_its_domain_and_passphrase() -> Result<(), Error> {
    let derived = derive(PASSPHRASE, DOMAIN)?.to_hexadecimal();

    assert_eq!(derive(PASSPHRASE, DOMAIN)?.to_hexadecimal(), derived);
    assert_ne!(
        derive(PASSPHRASE, b"another-domain")?.to_hexadecimal(),
        derived,
        "the domain did not separate the keys"
    );
    assert_ne!(
        derive(
            "another-high-entropy-passphrase-of-32-bytes-or-more",
            DOMAIN
        )?
        .to_hexadecimal(),
        derived,
        "a different passphrase derived the same key"
    );
    Ok(())
}

#[test]
#[ignore = "runs Argon2id, so run it in a release build: cargo test -p abnegate-secret --release --features derivation -- --ignored"]
fn a_derived_key_seals_and_opens_an_envelope() -> Result<(), Error> {
    let key = derive(PASSPHRASE, DOMAIN)?;
    let token = SecretValue::new(concat!("ghp_", "0123456789abcdefghij"));

    let sealed = encrypt_value(&token, &key)?;
    assert_eq!(decrypt_value(&sealed, &derive(PASSPHRASE, DOMAIN)?)?, token);
    Ok(())
}

#[test]
fn a_passphrase_is_derived_from_its_exact_bytes() -> Result<(), Error> {
    assert_eq!(
        derive(UNNORMALISED_PASSPHRASE, DOMAIN)?
            .to_hexadecimal()
            .as_str(),
        DERIVED_FROM_EXACT_BYTES
    );
    Ok(())
}

#[test]
fn an_empty_passphrase_is_refused() {
    assert!(matches!(derive("", DOMAIN), Err(Error::EmptyPassphrase)));
}
