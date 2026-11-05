use abnegate_secret::SecretValue;
use hmac::Hmac;
use hmac::KeyInit;
use hmac::Mac;
use sha2::Sha256;

/// What GitHub writes before the hex digest in [`WebhookSignature::HEADER`].
const PREFIX: &str = "sha256=";

/// The HMAC-SHA256 digest GitHub signs a webhook delivery's body with, read
/// from its [`WebhookSignature::HEADER`].
///
/// The digest is keyed with the secret the webhook was registered with, so a
/// delivery whose signature [`WebhookSignature::verifies`] came from someone
/// who holds that secret, and its body is the one they signed. The comparison
/// takes the same time however many bytes match, so how long a refusal takes
/// says nothing about the digest that would have been accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSignature(Vec<u8>);

impl WebhookSignature {
    /// The header GitHub sends a delivery's signature in.
    pub const HEADER: &str = "X-Hub-Signature-256";

    /// The digest in a [`WebhookSignature::HEADER`] value, or `None` when the
    /// value does not start with `sha256=`, spelled in lowercase, or what
    /// follows is not hex. The hex digits themselves may be in either case.
    pub fn parse(value: &str) -> Option<Self> {
        let digest = value.strip_prefix(PREFIX)?;
        hex::decode(digest).ok().map(Self)
    }

    /// Whether this is the digest of `body` keyed with `secret`.
    pub fn verifies(&self, secret: &SecretValue, body: &[u8]) -> bool {
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
        assert_eq!(WebhookSignature::parse("sha256=invalid"), None);
        assert_eq!(WebhookSignature::parse("sha256=abc"), None);
    }

    #[test]
    fn a_digest_without_its_prefix_does_not_parse() {
        assert_eq!(WebhookSignature::parse("abcdef1234567890"), None);
        let header = signed(&secret(), BODY);
        assert_eq!(WebhookSignature::parse(&header[PREFIX.len()..]), None);
    }

    #[test]
    fn a_prefix_spelled_in_uppercase_does_not_parse() {
        let header = signed(&secret(), BODY).replacen(PREFIX, "SHA256=", 1);

        assert_eq!(WebhookSignature::parse(&header), None);
    }

    #[test]
    fn a_signature_with_another_secret_does_not_verify() {
        let header = signed(&SecretValue::new("another webhook secret"), BODY);
        let signature = WebhookSignature::parse(&header).unwrap();

        assert!(!signature.verifies(&secret(), BODY));
    }

    #[test]
    fn a_truncated_digest_does_not_verify() {
        let header = signed(&secret(), BODY);
        let truncated = &header[..header.len() - 2];
        let signature = WebhookSignature::parse(truncated).unwrap();

        assert!(!signature.verifies(&secret(), BODY));
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
