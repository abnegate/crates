/// The implicit-TLS submissions port.
const SUBMISSIONS_PORT: u16 = 465;

/// How the connection to an SMTP relay is secured.
///
/// A relay with none chosen gets the one its port implies, from
/// [`SmtpSecurity::for_port`], and that is never [`SmtpSecurity::Plaintext`]:
/// sending in the clear is only ever an explicit choice.
///
/// ```
/// use abnegate_notify::SmtpSecurity;
///
/// assert_eq!(SmtpSecurity::for_port(465), SmtpSecurity::Implicit);
/// assert_eq!(SmtpSecurity::for_port(587), SmtpSecurity::StartTls);
/// assert_eq!(SmtpSecurity::for_port(25), SmtpSecurity::StartTls);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SmtpSecurity {
    /// TLS from the first byte, the way port 465 is served.
    Implicit,
    /// A plaintext connection upgraded with STARTTLS: the relay's greeting and
    /// the client's `EHLO` cross in the clear, everything after the upgrade is
    /// encrypted. A relay that cannot upgrade is never sent the credentials or
    /// the message, so a downgrade fails rather than leaks.
    StartTls,
    /// No encryption at all. **A config that signs in sends its user and
    /// password in the clear**, as `AUTH` over plaintext, and every message
    /// crosses the network unencrypted. Only for a relay on loopback or a
    /// network that is trusted end to end, preferably with
    /// [`SmtpConfig::without_credentials`](crate::SmtpConfig::without_credentials),
    /// and only ever chosen with
    /// [`SmtpConfig::with_security`](crate::SmtpConfig::with_security).
    Plaintext,
}

impl SmtpSecurity {
    /// The security a relay on `port` is reached with when none is chosen:
    /// implicit TLS on 465, STARTTLS on every other port.
    pub const fn for_port(port: u16) -> Self {
        if port == SUBMISSIONS_PORT {
            Self::Implicit
        } else {
            Self::StartTls
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_465_is_implicit_tls() {
        assert_eq!(SmtpSecurity::for_port(465), SmtpSecurity::Implicit);
    }

    #[test]
    fn every_other_port_upgrades_with_starttls() {
        for port in [25, 587, 2525, 0, u16::MAX] {
            assert_eq!(
                SmtpSecurity::for_port(port),
                SmtpSecurity::StartTls,
                "{port}"
            );
        }
    }
}
