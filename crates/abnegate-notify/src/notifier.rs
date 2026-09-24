//! The one trait every channel implements.

use std::time::Duration;

use async_trait::async_trait;

use crate::channel::Channel;
use crate::error::Error;
use crate::notification::Notification;

/// A single delivery destination.
///
/// One method, taking the whole payload, so a new kind of notification is a
/// new [`Notification`] rather than a new trait method every existing backend
/// has to grow. Adding a channel is then one file: implement this, hand it to
/// a [`Fanout`](crate::Fanout).
#[async_trait]
pub trait Notifier: Send + Sync + 'static {
    /// Which kind of destination this is.
    fn channel(&self) -> Channel;

    /// Which configured instance this is, when one channel is set up twice.
    ///
    /// `None` reports the delivery under the channel name, which is right
    /// until a caller has both an alerts Slack and an engineering Slack and
    /// needs to know which of the two failed.
    fn name(&self) -> Option<&str> {
        None
    }

    /// How long this backend should be given, overriding the fan-out default.
    fn timeout(&self) -> Option<Duration> {
        None
    }

    async fn deliver(&self, notification: &Notification) -> Result<(), Error>;
}
