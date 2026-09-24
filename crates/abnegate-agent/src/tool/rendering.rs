use std::ops::Range;

/// What opens and closes a [code span](Rendering::code).
pub(super) const BACKTICK: char = '`';

/// What a tool says a call will do, for a [`Preview`](super::Preview) to draw.
///
/// The tool's own words and the call's text set among them, drawn verbatim
/// but for the preview's escapes and never squeezed. Call text whose extent
/// the reader has to see, a command or the directory it runs in, goes in a
/// [code span](Self::code).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendering {
    text: String,
    /// Where in `text` each code span's content lies, in order.
    spans: Vec<Range<usize>>,
}

impl Rendering {
    /// `text` added as it is.
    pub fn text(mut self, text: &str) -> Self {
        self.text.push_str(text);
        self
    }

    /// `code` added between backticks.
    ///
    /// A backtick `code` holds is escaped on the card, so no call can close
    /// the span it is shown in and write the words the card goes on with: an
    /// `in DIR` clause after a command, or a second sentence of its own.
    pub fn code(mut self, code: &str) -> Self {
        self.text.push(BACKTICK);
        let start = self.text.len();
        self.text.push_str(code);
        self.spans.push(start..self.text.len());
        self.text.push(BACKTICK);
        self
    }

    /// Every character in order, with whether it lies inside a code span.
    pub(super) fn characters(&self) -> impl Iterator<Item = (char, bool)> + '_ {
        let mut spans = self.spans.iter().peekable();
        self.text.char_indices().map(move |(index, character)| {
            while spans.next_if(|span| span.end <= index).is_some() {}
            let inside = spans.peek().is_some_and(|span| span.contains(&index));
            (character, inside)
        })
    }
}

impl From<String> for Rendering {
    fn from(text: String) -> Self {
        Self {
            text,
            spans: Vec::new(),
        }
    }
}

impl From<&str> for Rendering {
    fn from(text: &str) -> Self {
        Self::from(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(rendering: &Rendering) -> String {
        rendering
            .characters()
            .filter_map(|(character, inside)| inside.then_some(character))
            .collect()
    }

    #[test]
    fn a_code_span_is_drawn_between_backticks_and_only_its_content_lies_inside() {
        let rendering = Rendering::from("In ")
            .code("a`b")
            .text(", run ")
            .code("")
            .code("ls")
            .text(".");

        assert_eq!(rendering.text, "In `a`b`, run ```ls`.");
        assert_eq!(inside(&rendering), "a`bls");
        assert_eq!(inside(&Rendering::from("no `spans` here")), "");
    }
}
