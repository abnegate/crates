//! A subscriber for tests that must evaluate every log line.
//!
//! With no subscriber installed a `tracing` macro never evaluates its
//! arguments, so a panic hiding in a log argument would never fire in a test.

use tracing::Event;
use tracing::Metadata;
use tracing::Subscriber;
use tracing::span::Attributes;
use tracing::span::Id;
use tracing::span::Record;

const SPAN: u64 = 1;

/// Enables every callsite and discards what it is sent.
pub(crate) struct Listening;

impl Subscriber for Listening {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(SPAN)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, _event: &Event<'_>) {}

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}
