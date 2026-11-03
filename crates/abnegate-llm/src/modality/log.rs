use crate::modality::Exchange;

/// Every exchange a client has recorded, and how many of them have been handed
/// out as unrecorded. It only grows, so the cursor never passes its end.
#[derive(Debug, Default)]
pub(super) struct Log {
    exchanges: Vec<Exchange>,
    recorded: usize,
}

impl Log {
    /// Keep `exchange` at the end of the log.
    pub(super) fn push(&mut self, exchange: Exchange) {
        self.exchanges.push(exchange);
    }

    /// Every exchange so far, oldest first.
    pub(super) fn exchanges(&self) -> &[Exchange] {
        &self.exchanges
    }

    /// The exchanges pushed since the last call, oldest first.
    pub(super) fn unrecorded(&mut self) -> Vec<Exchange> {
        let unrecorded = self
            .exchanges
            .get(self.recorded..)
            .unwrap_or_default()
            .to_vec();
        self.recorded = self.exchanges.len();
        unrecorded
    }
}

#[cfg(test)]
mod tests {
    use super::Log;
    use crate::modality::Exchange;

    fn exchange(answer: &str) -> Exchange {
        Exchange {
            provider: "mock".into(),
            model: "mock-model".into(),
            schema: None,
            response_schema: None,
            system_prompt: String::new(),
            user_prompt: String::new(),
            answer: answer.into(),
            input_tokens: 0,
            output_tokens: 0,
            attempts: 1,
        }
    }

    #[test]
    fn unrecorded_moves_the_cursor_without_shrinking_the_log() {
        let mut log = Log::default();
        log.push(exchange("first"));

        assert_eq!(log.unrecorded(), [exchange("first")]);

        log.push(exchange("second"));

        assert_eq!(log.unrecorded(), [exchange("second")]);
        assert!(log.unrecorded().is_empty());
        assert_eq!(log.exchanges(), [exchange("first"), exchange("second")]);
    }
}
