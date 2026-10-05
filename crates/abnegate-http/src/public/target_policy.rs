use crate::error::Result;
use crate::metadata;
use crate::url::validate_public_url;
use reqwest::Url;

/// Which targets a [`PublicClient`](crate::PublicClient) refuses, by URL,
/// by resolved address and on every redirect hop.
///
/// Both policies refuse a scheme other than `http` or `https`, a URL that
/// embeds credentials and a URL with no host.
///
/// ```
/// use abnegate_http::{Error, TargetPolicy};
///
/// assert!(matches!(
///     TargetPolicy::Public.validate("http://127.0.0.1:11434/v1"),
///     Err(Error::PrivateAddress)
/// ));
/// assert!(TargetPolicy::ExceptMetadata.validate("http://127.0.0.1:11434/v1").is_ok());
/// assert!(matches!(
///     TargetPolicy::ExceptMetadata.validate("http://169.254.169.254/latest"),
///     Err(Error::MetadataTarget)
/// ));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TargetPolicy {
    /// Refuse every target inside the deployment, which suits a URL from the
    /// open internet: whatever [`validate_public_url`] refuses, and every name
    /// that resolves only to such addresses.
    #[default]
    Public,

    /// Refuse only link-local addresses and the cloud metadata services, which
    /// suits an endpoint an operator or a tenant configured, such as a
    /// self-hosted model server on loopback, the LAN or a single-label host:
    /// whatever [`metadata::is_url`] refuses, and every name that resolves only
    /// to such addresses.
    ExceptMetadata,
}

impl TargetPolicy {
    /// Parse `raw` and refuse it if this policy refuses its target.
    pub fn validate(self, raw: &str) -> Result<Url> {
        match self {
            Self::Public => validate_public_url(raw),
            Self::ExceptMetadata => metadata::validate(raw),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn the_default_policy_is_public() {
        assert_eq!(TargetPolicy::default(), TargetPolicy::Public);
    }

    #[test]
    fn only_except_metadata_reaches_inside_the_deployment() {
        for inside in [
            "http://127.0.0.1:4000/v1",
            "http://localhost:11434/v1",
            "http://10.0.0.5/v1",
            "http://printer.local/",
            "http://gateway.internal/v1",
        ] {
            assert!(TargetPolicy::Public.validate(inside).is_err(), "{inside}");
            assert!(
                TargetPolicy::ExceptMetadata.validate(inside).is_ok(),
                "{inside}"
            );
        }
    }

    #[test]
    fn both_policies_refuse_link_local_metadata_targets() {
        for metadata in [
            "http://169.254.169.254/latest",
            "http://[fe80::1]/v1",
            "http://[fd00:ec2::254]/v1",
            "http://100.100.100.200/latest",
        ] {
            assert!(
                matches!(
                    TargetPolicy::Public.validate(metadata),
                    Err(Error::PrivateAddress)
                ),
                "{metadata}"
            );
            assert!(
                matches!(
                    TargetPolicy::ExceptMetadata.validate(metadata),
                    Err(Error::MetadataTarget)
                ),
                "{metadata}"
            );
        }
    }
}
