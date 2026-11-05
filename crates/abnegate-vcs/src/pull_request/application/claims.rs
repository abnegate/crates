use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::ApplicationId;
use abnegate_secret::SecretValue;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::Algorithm;
use jsonwebtoken::EncodingKey;
use jsonwebtoken::Header;
use jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER;
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// The algorithm GitHub requires an App's JWT to be signed with.
const ALGORITHM: Algorithm = Algorithm::RS256;

/// The claims of a JWT that authenticates as a GitHub App itself, rather than
/// as one of its installations.
///
/// Each time is whole seconds since the Unix epoch, as GitHub reads a JWT's,
/// and is negative before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationClaims {
    #[serde(rename = "iat")]
    issued_at: i64,
    #[serde(rename = "exp")]
    expires_at: i64,
    #[serde(rename = "iss")]
    issuer: String,
}

impl ApplicationClaims {
    /// How far a JWT's issue time is set back from now.
    ///
    /// GitHub refuses a JWT issued in the future by its own clock, and a
    /// server a few seconds fast is enough to be refused, so every JWT is
    /// issued as of a minute ago.
    pub const CLOCK_SKEW: Duration = Duration::from_secs(60);

    /// How long a JWT stays valid from its issue time.
    ///
    /// GitHub caps a JWT's lifetime at ten minutes and refuses one past it.
    /// The lifetime runs from the set-back issue time, so setting it back
    /// cannot eat into it, and nine minutes leaves a minute of headroom.
    pub const LIFETIME: Duration = Duration::from_mins(9);

    /// The claims of a JWT `application` signs at `now`: issued
    /// [`Self::CLOCK_SKEW`] before `now` and expiring [`Self::LIFETIME`] after
    /// that. A `now` before the Unix epoch gives negative times rather than a
    /// panic, and one too far from it for the times to be held saturates.
    pub fn issue(application: ApplicationId, now: SystemTime) -> Self {
        let issued_at = now
            .checked_sub(Self::CLOCK_SKEW)
            .map_or(i64::MIN, seconds_since_epoch);
        Self {
            issued_at,
            expires_at: issued_at.saturating_add(whole_seconds(Self::LIFETIME)),
            issuer: application.get().to_string(),
        }
    }

    /// When the JWT was issued, in seconds since the Unix epoch: its `iat`.
    pub fn issued_at(&self) -> i64 {
        self.issued_at
    }

    /// When the JWT stops being valid, in seconds since the Unix epoch: its
    /// `exp`.
    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }

    /// The App that issued the JWT, as its identifier in decimal: its `iss`.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// These claims signed RS256 with the App's `private_key`, an RSA key in
    /// PEM form.
    ///
    /// The JWT is a bearer credential for the whole App, so it is a
    /// [`SecretValue`] and never a `String`. Neither failure keeps the
    /// signer's own error, whose message can quote the key it failed to
    /// read. It is signed through aws-lc-rs by name rather than through
    /// `jsonwebtoken`'s process-wide default provider, which panics when
    /// feature unification enables both of its backends.
    pub fn sign(&self, private_key: &SecretValue) -> Result<SecretValue, ApplicationError> {
        let key = EncodingKey::from_rsa_pem(private_key.expose().as_bytes())
            .map_err(|_| ApplicationError::PrivateKey)?;
        let signer = (DEFAULT_PROVIDER.signer_factory)(&ALGORITHM, &key)
            .map_err(|_| ApplicationError::PrivateKey)?;
        let message = format!("{}.{}", encoded(&Header::new(ALGORITHM))?, encoded(self)?);
        let signature = signer
            .try_sign(message.as_bytes())
            .map_err(|_| ApplicationError::Signing)?;
        Ok(SecretValue::new(format!(
            "{message}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        )))
    }
}

/// `part` of a JWT, its header or its claims, as base64url JSON without
/// padding.
fn encoded(part: &impl Serialize) -> Result<String, ApplicationError> {
    serde_json::to_vec(part)
        .map(|json| URL_SAFE_NO_PAD.encode(json))
        .map_err(|_| ApplicationError::Signing)
}

/// `instant` in whole seconds since the Unix epoch, rounded down, so negative
/// before it.
fn seconds_since_epoch(instant: SystemTime) -> i64 {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => whole_seconds(since),
        Err(before) => {
            let before = before.duration();
            let partial = i64::from(before.subsec_nanos() > 0);
            whole_seconds(before)
                .saturating_neg()
                .saturating_sub(partial)
        }
    }
}

/// The whole seconds in `duration`, saturating where they outgrow an `i64`.
fn whole_seconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::fixtures::PRIVATE_KEY;
    use crate::pull_request::application::fixtures::PUBLIC_KEY;
    use crate::pull_request::application::fixtures::at;
    use jsonwebtoken::DecodingKey;
    use jsonwebtoken::crypto::CryptoProvider;
    use jsonwebtoken::crypto::KeyUtils;
    use serde_json::Value;

    /// An instant well after the Unix epoch.
    const NOW: u64 = 1_700_000_000;

    /// `openssl dgst -sha256 -sign` over this JWT's header and claims with
    /// [`PRIVATE_KEY`], so the signature it carries owes nothing to
    /// `jsonwebtoken`.
    ///
    /// Its header bytes, `{"typ":"JWT","alg":"RS256"}`, are pinned on
    /// purpose: GitHub reads the header's fields in any order, but the
    /// signature covers its exact bytes, so a change to how the header is
    /// written has to be made here deliberately rather than slip through.
    const OPENSSL_SIGNED_JWT: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.eyJpYXQiOjE2OTk5OTk5NDAsImV4cCI6MTcwMDAwMDQ4MCwiaXNzIjoiMTIzNDUifQ.s-lV36kULTAVFysUYkpQT7-REG0frHVaOrcQZy3zk60Ob_p-SOh4ESN6hjsS3y4qwcuJAPNsT5lsLJg7lYabSpNflnqJlqHNACgkwgCk-Er8GTLV2ENU_hFcY4g6BcW7-ZyKo6bMDsR6ZnhIZNqGwOonNAT31WQnmePvTVUekTQvkGUA4oLHdsEdIdiCdHSR9z8jLfUanjy1aYqhupr1ppAFsrw1U_InDKN0urpCrTnUavd5lucjJqPBGIasPOHuT5tfUnLbbYxnmAE4aQwcByI92MUrxskfwSTc3J4y2kePVUUesZX99gfSKKfyTN87Pwi7thLnlpigfjWh8GAkcg";

    /// A process-default provider that refuses to sign, standing in for the
    /// one `jsonwebtoken` falls back to when both of its backends are
    /// enabled, which panics the same way.
    static REFUSING: CryptoProvider = CryptoProvider {
        signer_factory: |_, _| panic!("signing consulted the process-default CryptoProvider"),
        verifier_factory: |algorithm, key| (DEFAULT_PROVIDER.verifier_factory)(algorithm, key),
        key_utils: KeyUtils::new_unimplemented(),
    };

    fn application() -> ApplicationId {
        ApplicationId::new(12345)
    }

    fn signed(now: SystemTime) -> SecretValue {
        ApplicationClaims::issue(application(), now)
            .sign(&SecretValue::new(PRIVATE_KEY))
            .unwrap()
    }

    fn part(jwt: &str, index: usize) -> Value {
        let encoded = jwt.split('.').nth(index).unwrap();
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap()
    }

    /// The claims of `jwt` when the App's public key verifies its RS256
    /// signature, through aws-lc-rs by name, as [`ApplicationClaims::sign`]
    /// signs, so no test reads the process-default provider.
    fn verify(jwt: &str) -> Option<ApplicationClaims> {
        let (message, signature) = jwt.rsplit_once('.')?;
        let key = DecodingKey::from_rsa_pem(PUBLIC_KEY.as_bytes()).unwrap();
        let verifier = (DEFAULT_PROVIDER.verifier_factory)(&ALGORITHM, &key).unwrap();
        let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
        verifier.verify(message.as_bytes(), &signature).ok()?;
        serde_json::from_value(part(jwt, 1)).ok()
    }

    #[test]
    fn issued_at_is_backdated_for_clock_skew() {
        let claims = ApplicationClaims::issue(application(), at(NOW));

        assert_eq!(claims.issued_at(), 1_700_000_000 - 60);
        assert!(
            claims.issued_at() < 1_700_000_000,
            "a JWT issued in the future is rejected by GitHub"
        );
    }

    #[test]
    fn expiry_is_bounded_by_githubs_ten_minute_cap() {
        let claims = ApplicationClaims::issue(application(), at(NOW));

        assert_eq!(claims.expires_at() - claims.issued_at(), 9 * 60);
        assert!(
            claims.expires_at() - claims.issued_at() <= 10 * 60,
            "GitHub refuses a JWT whose lifetime exceeds ten minutes"
        );
    }

    #[test]
    fn expiry_is_still_in_the_future_after_backdating() {
        let claims = ApplicationClaims::issue(application(), at(NOW));

        assert!(claims.expires_at() > 1_700_000_000);
    }

    #[test]
    fn issuer_is_the_application_id() {
        assert_eq!(
            ApplicationClaims::issue(application(), at(NOW)).issuer(),
            "12345"
        );
    }

    #[test]
    fn a_time_before_the_epoch_is_issued_without_panicking() {
        let claims = ApplicationClaims::issue(application(), UNIX_EPOCH - Duration::from_secs(30));
        assert_eq!(claims.issued_at(), -90);
        assert_eq!(claims.expires_at(), -90 + 9 * 60);

        let partial =
            ApplicationClaims::issue(application(), UNIX_EPOCH - Duration::from_millis(500));
        assert_eq!(
            partial.issued_at(),
            -61,
            "a time part way through a second is rounded down"
        );
    }

    #[test]
    fn a_signed_jwt_verifies_with_the_apps_public_key() {
        let now = SystemTime::now();

        let claims = verify(signed(now).expose())
            .expect("GitHub verifies the JWT with the App's public key");

        assert_eq!(claims, ApplicationClaims::issue(application(), now));
    }

    #[test]
    fn the_signature_matches_an_independent_pkcs1_v1_5_signer() {
        assert_eq!(
            signed(at(NOW)).expose(),
            OPENSSL_SIGNED_JWT,
            "RS256 is deterministic RSASSA-PKCS1-v1_5 with SHA-256, so OpenSSL must produce the same bytes"
        );
    }

    #[test]
    fn signing_never_consults_the_process_default_provider() {
        assert!(
            CryptoProvider::install_default(&REFUSING).is_ok(),
            "no other test in this binary may install or read jsonwebtoken's process default"
        );

        assert_eq!(
            signed(at(NOW)).expose(),
            OPENSSL_SIGNED_JWT,
            "signing goes through aws-lc-rs by name, whatever the process default is"
        );
    }

    #[test]
    fn the_openssl_signed_jwt_verifies_with_the_apps_public_key() {
        let claims = verify(OPENSSL_SIGNED_JWT).expect("OpenSSL's signature verifies");

        assert_eq!(claims, ApplicationClaims::issue(application(), at(NOW)));
    }

    #[test]
    fn a_jwt_with_forged_claims_fails_verification() {
        let jwt = signed(SystemTime::now());
        let forged = URL_SAFE_NO_PAD.encode(r#"{"iat":0,"exp":4102444800,"iss":"12345"}"#);
        let mut parts: Vec<&str> = jwt.expose().split('.').collect();
        parts[1] = &forged;

        assert!(
            verify(&parts.join(".")).is_none(),
            "a signature must not carry over to claims it did not sign"
        );
    }

    #[test]
    fn a_signed_jwt_has_three_parts_and_an_rs256_header() {
        let jwt = signed(SystemTime::now());

        assert_eq!(jwt.expose().split('.').count(), 3);
        assert_eq!(part(jwt.expose(), 0)["alg"], "RS256");
    }

    #[test]
    fn the_claims_carry_a_backdated_iat_and_a_bounded_exp() {
        let jwt = signed(at(NOW));
        let claims = part(jwt.expose(), 1);

        assert_eq!(claims["iss"], "12345");
        assert_eq!(claims["iat"].as_i64().unwrap(), 1_700_000_000 - 60);
        assert_eq!(
            claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
            9 * 60
        );
    }

    #[test]
    fn the_jwt_never_renders_itself() {
        let jwt = signed(SystemTime::now());

        assert_eq!(format!("{jwt:?}"), "[REDACTED]");
        assert_eq!(jwt.to_string(), "[REDACTED]");
        assert!(!format!("{jwt:?}").contains('.'));
    }

    #[test]
    fn a_broken_private_key_is_rejected_without_quoting_it() {
        let error = ApplicationClaims::issue(application(), SystemTime::now())
            .sign(&SecretValue::new(
                "-----BEGIN RSA PRIVATE KEY-----\nnot-a-key\n-----END RSA PRIVATE KEY-----",
            ))
            .expect_err("an unparseable key cannot sign");

        assert!(matches!(error, ApplicationError::PrivateKey));
        assert!(!error.to_string().contains("not-a-key"));
    }
}
