use argon2::Algorithm;
use argon2::Argon2;
use argon2::Block;
use argon2::Params;
use argon2::Version;
use sha2::Digest;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::Error;
use crate::key::KEY_BYTES;
use crate::key::MasterKey;
use crate::value::SecretValue;

const SALT_BYTES: usize = 16;
const DIGEST_BYTES: usize = 32;
const MEMORY_KIBIBYTES: u32 = 19_456;
const PASSES: u32 = 2;
const LANES: u32 = 1;

const PARAMETERS: Params = match Params::new(MEMORY_KIBIBYTES, PASSES, LANES, Some(KEY_BYTES)) {
    Ok(parameters) => parameters,
    Err(_) => panic!("the Argon2id parameters are outside the limits argon2 accepts"),
};
const BLOCKS: usize = PARAMETERS.block_count();

/// The domain [`MasterKey::derive`] stretches a passphrase into a key under.
#[cfg_attr(docsrs, doc(cfg(feature = "derivation")))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyDerivation {
    domain: Vec<u8>,
}

impl KeyDerivation {
    /// Derive keys under `domain`.
    ///
    /// The domain is hashed into the salt, the first 16 bytes of the SHA-256
    /// of the domain followed by the passphrase, so the same passphrase
    /// derives an unrelated key under every other domain. Name one per
    /// application and purpose, and change it only to start over with new
    /// keys.
    ///
    /// Every other parameter is fixed, so a key derived today is the key every
    /// later release derives: Argon2id, version 0x13, 19456 KiB of memory, 2
    /// passes, 1 lane and a 32-byte output.
    pub fn new(domain: &[u8]) -> Self {
        Self {
            domain: domain.to_vec(),
        }
    }

    fn salt(&self, passphrase: &[u8]) -> Zeroizing<[u8; SALT_BYTES]> {
        let mut digest = Zeroizing::new([0u8; DIGEST_BYTES]);
        let mut hasher = Sha256::new();
        hasher.update(&self.domain);
        hasher.update(passphrase);
        hasher.finalize_into((&mut *digest).into());

        let mut salt = Zeroizing::new([0u8; SALT_BYTES]);
        salt.copy_from_slice(&digest[..SALT_BYTES]);
        salt
    }
}

impl MasterKey {
    /// Stretch `passphrase` into a key with Argon2id, under the domain
    /// `derivation` names.
    ///
    /// The same passphrase and domain always derive the same key. The salt
    /// comes from the passphrase itself, so the key is only as strong as the
    /// passphrase: the caller is responsible for its entropy, and this crate
    /// imposes no length limit. Argon2id is memory-hard on purpose, so this
    /// takes a noticeable moment, and far longer in an unoptimised build.
    ///
    /// Fails with [`Error::Derivation`] when Argon2id cannot allocate its
    /// working memory or refuses the passphrase's length.
    ///
    /// ```no_run
    /// use abnegate_secret::KeyDerivation;
    /// use abnegate_secret::MasterKey;
    /// use abnegate_secret::SecretValue;
    ///
    /// let derivation = KeyDerivation::new(b"example-application-v1");
    /// let passphrase = SecretValue::new("a long passphrase drawn from a good random source");
    /// let key = MasterKey::derive(&passphrase, &derivation)?;
    /// # Ok::<(), abnegate_secret::Error>(())
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "derivation")))]
    pub fn derive(passphrase: &SecretValue, derivation: &KeyDerivation) -> Result<Self, Error> {
        let passphrase = passphrase.expose().as_bytes();
        let salt = derivation.salt(passphrase);
        let mut memory: Zeroizing<Vec<Block>> = Zeroizing::new(Vec::new());
        memory
            .try_reserve_exact(BLOCKS)
            .map_err(|_| Error::Derivation)?;
        memory.resize(BLOCKS, Block::default());

        let mut derived = Self {
            key: [0u8; KEY_BYTES],
        };
        Argon2::new(Algorithm::Argon2id, Version::V0x13, PARAMETERS)
            .hash_password_into_with_memory(
                passphrase,
                salt.as_slice(),
                &mut derived.key,
                memory.as_mut_slice(),
            )
            .map_err(|_| Error::Derivation)?;
        Ok(derived)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &[u8] = b"abnegate-secret-derivation-v1";
    const PASSPHRASE: &[u8] = b"a-high-entropy-passphrase-of-at-least-32-bytes";
    const SALT: &str = "c4ce99a418bae57d999e34f514fe0cfd";

    #[test]
    fn the_salt_is_the_first_sixteen_bytes_of_the_domain_and_passphrase_digest() {
        let salt = KeyDerivation::new(DOMAIN).salt(PASSPHRASE);
        assert_eq!(hex::encode(salt.as_slice()), SALT);
    }

    #[test]
    fn a_different_domain_gives_a_different_salt() {
        let salt = KeyDerivation::new(DOMAIN).salt(PASSPHRASE);
        let other = KeyDerivation::new(b"another-domain").salt(PASSPHRASE);
        assert_ne!(salt, other);
    }
}
