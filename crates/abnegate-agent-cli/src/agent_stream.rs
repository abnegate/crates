//! A running agent's events, as it produces them.

use std::pin::Pin;

use abnegate_llm::ProviderError;
use futures::Stream;

use crate::event::AgentEvent;

/// A coding agent's events, yielded as it emits them, from
/// [`CliProvider::stream`](crate::CliProvider::stream).
///
/// The stream ends once the agent has exited or been stopped. A run that
/// failed in any way ends it with one `Err`, the error
/// [`CompletionProvider::complete`](abnegate_llm::CompletionProvider::complete)
/// would have returned; a failure the agent reported is that error and is
/// never yielded as an [`AgentEvent::Failed`], and a turn a usage limit
/// refused ends it in a [`ProviderError::Limited`] rather than yielding an
/// [`AgentEvent::Limited`]. A Claude stream that ends on a refused window
/// with no line after it ends in a [`ProviderError::Agent`] in the words the
/// limit would have had. Dropping the stream while the agent still runs
/// stops it and every process it forked.
pub type AgentStream = Pin<Box<dyn Stream<Item = Result<AgentEvent, ProviderError>> + Send>>;
