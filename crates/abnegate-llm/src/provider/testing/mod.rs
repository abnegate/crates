//! A provider whose answers a test decides, for this crate's tests and for a
//! crate layered on top of it that routes or wraps completion providers.

mod behaviour;
mod seen;
mod stub;

pub use crate::provider::testing::behaviour::Behaviour;
pub use crate::provider::testing::seen::Seen;
pub use crate::provider::testing::stub::StubProvider;
