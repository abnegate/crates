/// The request field a completion's output budget travels in.
///
/// OpenAI's reasoning models refuse `max_tokens` and take the budget as
/// `max_completion_tokens`; every other endpoint reads `max_tokens`.
/// [`Dialect::budget`](crate::Dialect::budget) says which one a model takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Budget {
    /// Sent as `max_tokens`.
    MaximumTokens,
    /// Sent as `max_completion_tokens`.
    MaximumCompletionTokens,
}
