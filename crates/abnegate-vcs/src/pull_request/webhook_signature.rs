use abnegate_secret::SecretValue;
use hmac::Hmac;
use hmac::KeyInit;
use hmac::Mac;
use sha2::Sha256;

/// What GitHub writes before the hex digest in [`WebhookSignature::HEADER`].
const PREFIX: &str = "sha256=";

/// How many bytes an HMAC-SHA256 digest holds.
const DIGEST_BYTES: usize = 32;

/// The HMAC-SHA256 digest GitHub signs a webhook delivery's body with, read
/// from its [`WebhookSignature::HEADER`].
///
/// The digest is keyed with the secret the webhook was registered with, so a
/// delivery whose signature [`WebhookSignature::verifies`] came from someone
/// who holds that secret, and its body is the one they signed. The comparison
/// takes the same time however many bytes match, so how long a refusal takes
/// says nothing about the digest that would have been accepted. It has no
/// `==`, which would not take the same time.
#[derive(Debug, Clone)]
pub struct WebhookSignature([u8; DIGEST_BYTES]);

impl WebhookSignature {
    /// The header GitHub sends a delivery's signature in.
    pub const HEADER: &str = "X-Hub-Signature-256";

    /// The digest in a [`WebhookSignature::HEADER`] value, or `None` when the
    /// value does not start with `sha256=`, spelled in lowercase, or what
    /// follows is not the 64 hex digits of a SHA-256 digest. The hex digits
    /// themselves may be in either case.
    pub fn parse(value: &str) -> Option<Self> {
        let digest = value.strip_prefix(PREFIX)?;
        let mut bytes = [0; DIGEST_BYTES];
        hex::decode_to_slice(digest, &mut bytes).ok()?;
        Some(Self(bytes))
    }

    /// Whether this is the digest of `body` keyed with `secret`. An empty
    /// `secret` verifies nothing, since anyone can key a digest with it.
    pub fn verifies(&self, secret: &SecretValue, body: &[u8]) -> bool {
        if secret.expose().is_empty() {
            return false;
        }
        Hmac::<Sha256>::new_from_slice(secret.expose().as_bytes())
            .map(|mac| mac.chain_update(body).verify_slice(&self.0).is_ok())
            .unwrap_or(false)
    }

    /// Whether `header`, a [`WebhookSignature::HEADER`] value, signs `body`
    /// with `secret`: `false` when it does not parse, as well as when it
    /// does not verify.
    pub fn verify(header: &str, secret: &SecretValue, body: &[u8]) -> bool {
        Self::parse(header).is_some_and(|signature| signature.verifies(secret, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = b"test payload";

    fn secret() -> SecretValue {
        SecretValue::new("a webhook secret")
    }

    fn signed(secret: &SecretValue, body: &[u8]) -> String {
        let digest = Hmac::<Sha256>::new_from_slice(secret.expose().as_bytes())
            .unwrap()
            .chain_update(body)
            .finalize()
            .into_bytes();
        format!("{PREFIX}{}", hex::encode(digest))
    }

    #[test]
    fn a_signature_of_the_body_with_the_secret_verifies() {
        let header = signed(&secret(), BODY);
        let signature = WebhookSignature::parse(&header).unwrap();

        assert!(signature.verifies(&secret(), BODY));
        assert!(
            !signature.verifies(&secret(), b"another payload"),
            "a signature verifies only the body it signed"
        );
        let uppercase = format!("{PREFIX}{}", header[PREFIX.len()..].to_uppercase());
        assert!(
            WebhookSignature::parse(&uppercase)
                .unwrap()
                .verifies(&secret(), BODY),
            "a digest reads the same in either case"
        );
    }

    #[test]
    fn a_digest_that_is_not_hex_does_not_parse() {
        assert!(WebhookSignature::parse("sha256=invalid").is_none());
        assert!(WebhookSignature::parse("sha256=abc").is_none());
        let header = signed(&secret(), BODY);
        let unhex = format!("{}zz", &header[..header.len() - 2]);
        assert!(
            WebhookSignature::parse(&unhex).is_none(),
            "a digest of the right length must still be hex"
        );
    }

    #[test]
    fn a_digest_of_the_wrong_length_does_not_parse() {
        let digest = &signed(&secret(), BODY)[PREFIX.len()..];

        for refused in [
            PREFIX.to_string(),
            format!("{PREFIX}{digest}00"),
            format!("{PREFIX}{}", &digest[..digest.len() - 2]),
        ] {
            assert!(WebhookSignature::parse(&refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn a_digest_without_its_prefix_does_not_parse() {
        assert!(WebhookSignature::parse("abcdef1234567890").is_none());
        let header = signed(&secret(), BODY);
        assert!(WebhookSignature::parse(&header[PREFIX.len()..]).is_none());
    }

    #[test]
    fn a_prefix_spelled_in_uppercase_does_not_parse() {
        let header = signed(&secret(), BODY).replacen(PREFIX, "SHA256=", 1);

        assert!(WebhookSignature::parse(&header).is_none());
    }

    #[test]
    fn a_signature_with_another_secret_does_not_verify() {
        let header = signed(&SecretValue::new("another webhook secret"), BODY);
        let signature = WebhookSignature::parse(&header).unwrap();

        assert!(!signature.verifies(&secret(), BODY));
    }

    #[test]
    fn a_truncated_digest_is_refused() {
        let header = signed(&secret(), BODY);
        let truncated = &header[..header.len() - 2];

        assert!(
            WebhookSignature::parse(truncated).is_none(),
            "a digest one byte short does not parse"
        );
        assert!(!WebhookSignature::verify(truncated, &secret(), BODY));
    }

    #[test]
    fn an_empty_secret_verifies_nothing() {
        let empty = SecretValue::new("");
        let header = signed(&empty, BODY);

        assert!(
            !WebhookSignature::parse(&header)
                .unwrap()
                .verifies(&empty, BODY),
            "a digest anyone can key proves nothing about who sent it"
        );
        assert!(!WebhookSignature::verify(&header, &empty, BODY));
    }

    #[test]
    fn a_signature_matches_the_vector_github_documents() {
        let secret = SecretValue::new("It's a Secret to Everybody");
        let header = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

        assert!(WebhookSignature::verify(header, &secret, b"Hello, World!"));
        assert_eq!(signed(&secret, b"Hello, World!"), header);
    }

    #[test]
    fn verify_reads_and_checks_a_header_in_one_step() {
        let header = signed(&secret(), BODY);

        assert!(WebhookSignature::verify(&header, &secret(), BODY));
        assert!(!WebhookSignature::verify(
            &header[PREFIX.len()..],
            &secret(),
            BODY
        ));
        assert!(!WebhookSignature::verify("sha256=invalid", &secret(), BODY));
        assert!(!WebhookSignature::verify(
            &header,
            &SecretValue::new("another webhook secret"),
            BODY
        ));
    }
}
