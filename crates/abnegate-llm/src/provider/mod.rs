//! Completion providers.
//!
//! A request runs against an OpenAI-compatible endpoint or, in a crate layered
//! on top of this one, shells out to a coding agent CLI, and nothing above this
//! module needs to know which. Both satisfy [`CompletionProvider`]; [`Router`]
//! satisfies it too, over a set of them, so selection, A/B splitting, and
//! fallback are invisible to the consumer.
//!
//! # Failure classification
//!
//! This module does not decide whether a failure is worth retrying. The caller
//! already owns that judgement and makes it by reading the failure text, so a
//! provider's job is to report what went wrong in the words the underlying
//! service used, with any credential scrubbed out.

mod capabilities;
mod completion;
mod completion_provider;
mod credential;
mod error;
mod exit_status;
mod http;
mod kind;
mod request;
mod router;
mod selection;
mod strategy;
mod weighted;

#[cfg(any(test, feature = "testing"))]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub mod testing;

pub use crate::provider::capabilities::Capabilities;
pub use crate::provider::completion::Completion;
pub use crate::provider::completion_provider::CompletionProvider;
pub use crate::provider::credential::Credential;
pub use crate::provider::error::ProviderError;
pub use crate::provider::exit_status::ExitStatus;
pub use crate::provider::http::HttpProvider;
pub use crate::provider::kind::ProviderKind;
pub use crate::provider::request::CompletionRequest;
pub use crate::provider::router::Router;
pub use crate::provider::selection::{choose, sample};
pub use crate::provider::strategy::SelectionStrategy;
pub use crate::provider::weighted::Weighted;
