//! Keeping the blocks of an agent's prose apart.

use crate::event::AgentEvent;

pub(crate) const PARAGRAPH_BREAK: &str = "\n\n";

/// Starts every block of an agent's prose after the first with a paragraph
/// break.
///
/// An agent writes a block between each round of tools, every one ending as
/// a sentence does, and run together they read as one paragraph with no
/// space after each full stop. A blank block is passed on as it is and does
/// not count as the first.
#[derive(Debug, Default)]
pub(crate) struct Paragraphs {
    spoken: bool,
}

impl Paragraphs {
    /// `event`, with a paragraph break ahead of its text when it is a block
    /// of prose that follows another.
    pub(crate) fn separated(&mut self, event: AgentEvent) -> AgentEvent {
        let AgentEvent::Text(text) = event else {
            return event;
        };
        if text.trim().is_empty() {
            return AgentEvent::Text(text);
        }
        if std::mem::replace(&mut self.spoken, true) {
            return AgentEvent::Text(format!("{PARAGRAPH_BREAK}{text}"));
        }
        AgentEvent::Text(text)
    }
}

#[cfg(test)]
mod tests {
    use abnegate_llm::Usage;

    use super::Paragraphs;
    use crate::event::AgentEvent;

    #[test]
    fn the_blocks_of_an_answer_are_kept_apart() {
        let mut paragraphs = Paragraphs::default();
        let spoken: Vec<AgentEvent> = [
            AgentEvent::Session("6f1".to_string()),
            AgentEvent::Text("  ".to_string()),
            AgentEvent::Text("I'll look first.".to_string()),
            AgentEvent::Usage(Usage::new(1, 1)),
            AgentEvent::Text("Found it.".to_string()),
        ]
        .into_iter()
        .map(|event| paragraphs.separated(event))
        .collect();

        let words: Vec<&str> = spoken
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(words, ["  ", "I'll look first.", "\n\nFound it."]);
    }

    #[test]
    fn a_final_answer_is_left_as_it_is() {
        let mut paragraphs = Paragraphs::default();
        paragraphs.separated(AgentEvent::Text("Looking now.".to_string()));

        let answer = paragraphs.separated(AgentEvent::Answer("Done.".to_string()));

        assert!(
            matches!(&answer, AgentEvent::Answer(text) if text == "Done."),
            "{answer:?}"
        );
    }
}
