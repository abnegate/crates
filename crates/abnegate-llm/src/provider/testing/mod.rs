//! Providers whose answers a test decides, for this crate's tests and for a
//! crate layered on top of it that routes or wraps completion providers or
//! ranks with a reranker.

mod behaviour;
mod seen;
mod stub;
mod stub_reranker;

pub use crate::provider::testing::behaviour::Behaviour;
pub use crate::provider::testing::seen::Seen;
pub use crate::provider::testing::stub::StubProvider;
pub use crate::provider::testing::stub_reranker::StubReranker;
