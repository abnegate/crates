//! What happened across every channel.

use crate::delivery::Delivery;

/// Every channel's outcome from one fan-out, in registration order.
///
/// A fan-out returns this instead of a `Result`, because "the Slack webhook
/// is misconfigured" and "nothing was delivered anywhere" are different
/// facts and a caller has to be able to tell them apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    deliveries: Vec<Delivery>,
}

impl Report {
    pub(crate) fn new(deliveries: Vec<Delivery>) -> Self {
        Self { deliveries }
    }

    pub fn deliveries(&self) -> &[Delivery] {
        &self.deliveries
    }

    /// The channels that took the notification.
    pub fn delivered(&self) -> impl Iterator<Item = &Delivery> {
        self.deliveries.iter().filter(|one| one.is_delivered())
    }

    /// The channels that did not.
    pub fn failures(&self) -> impl Iterator<Item = &Delivery> {
        self.deliveries.iter().filter(|one| !one.is_delivered())
    }

    /// The failures worth trying again.
    pub fn retryable(&self) -> impl Iterator<Item = &Delivery> {
        self.deliveries.iter().filter(|one| one.is_retryable())
    }

    /// How many channels were attempted.
    pub fn len(&self) -> usize {
        self.deliveries.len()
    }

    /// Whether no channel was configured.
    ///
    /// Distinct from [`Report::all_delivered`], which is vacuously true here:
    /// a fan-out with nothing registered is not a failure, but a caller that
    /// expected to reach someone will want to know it reached no one.
    pub fn is_empty(&self) -> bool {
        self.deliveries.is_empty()
    }

    pub fn all_delivered(&self) -> bool {
        self.deliveries.iter().all(Delivery::is_delivered)
    }

    pub fn any_delivered(&self) -> bool {
        self.deliveries.iter().any(Delivery::is_delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::Channel;
    use crate::error::NotifyError;
    use std::time::Duration;

    fn delivered(name: &str) -> Delivery {
        Delivery::new(Channel::SLACK, name.to_string(), Ok(()))
    }

    fn failed(name: &str) -> Delivery {
        Delivery::new(
            Channel::DISCORD,
            name.to_string(),
            Err(NotifyError::Timeout {
                after: Duration::from_secs(1),
            }),
        )
    }

    fn permanently_failed(name: &str) -> Delivery {
        Delivery::new(
            Channel::EMAIL,
            name.to_string(),
            Err(NotifyError::Malformed {
                message: "no recipients".to_string(),
            }),
        )
    }

    #[test]
    fn an_empty_report_is_not_a_failure() {
        let report = Report::default();
        assert!(report.is_empty());
        assert_eq!(report.len(), 0);
        assert!(report.all_delivered());
        assert!(!report.any_delivered());
        assert_eq!(report.failures().count(), 0);
    }

    #[test]
    fn a_mixed_report_separates_the_two_groups() {
        let report = Report::new(vec![delivered("slack"), failed("discord"), delivered("b")]);

        assert_eq!(report.len(), 3);
        assert!(!report.is_empty());
        assert!(!report.all_delivered());
        assert!(report.any_delivered());
        assert_eq!(report.delivered().count(), 2);

        let failures: Vec<&str> = report.failures().map(Delivery::name).collect();
        assert_eq!(failures, vec!["discord"]);
    }

    #[test]
    fn only_retryable_failures_are_offered_for_a_retry() {
        let report = Report::new(vec![failed("discord"), permanently_failed("email")]);
        let retryable: Vec<&str> = report.retryable().map(Delivery::name).collect();
        assert_eq!(retryable, vec!["discord"]);
        assert_eq!(report.failures().count(), 2);
    }
}
