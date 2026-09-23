//! What happened on one channel.

use crate::channel::Channel;
use crate::error::NotifyError;

/// The outcome of a single channel's attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    channel: Channel,
    name: String,
    outcome: Result<(), NotifyError>,
}

impl Delivery {
    pub(crate) fn new(channel: Channel, name: String, outcome: Result<(), NotifyError>) -> Self {
        Self {
            channel,
            name,
            outcome,
        }
    }

    pub fn channel(&self) -> &Channel {
        &self.channel
    }

    /// The configured instance name, falling back to the channel name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_delivered(&self) -> bool {
        self.outcome.is_ok()
    }

    pub fn error(&self) -> Option<&NotifyError> {
        self.outcome.as_ref().err()
    }

    /// Whether sending to this channel again could plausibly work.
    pub fn is_retryable(&self) -> bool {
        self.error().is_some_and(NotifyError::is_retryable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_delivered_notification_has_no_error() {
        let delivery = Delivery::new(Channel::SLACK, "slack".to_string(), Ok(()));
        assert!(delivery.is_delivered());
        assert!(delivery.error().is_none());
        assert!(!delivery.is_retryable());
        assert_eq!(delivery.channel(), &Channel::SLACK);
        assert_eq!(delivery.name(), "slack");
    }

    #[test]
    fn a_failure_carries_its_reason() {
        let delivery = Delivery::new(
            Channel::DISCORD,
            "discord".to_string(),
            Err(NotifyError::Timeout {
                after: Duration::from_secs(5),
            }),
        );
        assert!(!delivery.is_delivered());
        assert!(delivery.is_retryable());
        assert_eq!(
            delivery.error().map(ToString::to_string),
            Some("delivery timed out after 5000ms".to_string())
        );
    }
}
