use serde::Deserialize;

use crate::wire::stream_delta::StreamDelta;

/// One streaming choice.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct StreamChoice {
    /// Which answer this adds to, from 0.
    #[serde(default)]
    pub index: u32,
    /// What it adds.
    pub delta: StreamDelta,
    /// Why the model stopped, on the chunk that ends the answer.
    pub finish_reason: Option<String>,
}

impl StreamChoice {
    /// The answer at `index` gaining `delta`, with the answer not yet ended.
    pub fn new(index: u32, delta: StreamDelta) -> Self {
        Self {
            index,
            delta,
            finish_reason: None,
        }
    }

    /// The same choice, ending the answer for `finish_reason`.
    pub fn with_finish_reason(mut self, finish_reason: impl Into<String>) -> Self {
        self.finish_reason = Some(finish_reason.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::StreamChoice;
    use crate::wire::stream_delta::StreamDelta;

    #[test]
    fn a_new_choice_has_not_ended_until_given_a_reason() {
        let choice = StreamChoice::new(2, StreamDelta::default());
        assert_eq!(choice.index, 2);
        assert!(choice.finish_reason.is_none());

        let ended = choice.with_finish_reason("stop");
        assert_eq!(ended.finish_reason.as_deref(), Some("stop"));
    }
}
