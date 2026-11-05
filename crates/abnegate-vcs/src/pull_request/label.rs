/// A label on an issue, as its name and, where GitHub says, its colour and
/// what it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Label {
    /// What the label is called.
    pub name: String,
    /// Its colour as six hexadecimal digits without a `#`, if GitHub says.
    pub color: Option<String>,
    /// What it is for, if GitHub says.
    pub description: Option<String>,
}

impl Label {
    /// The label called `name`, with no colour or description.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            color: None,
            description: None,
        }
    }

    /// Its colour as six hexadecimal digits without a `#`.
    #[must_use]
    pub fn with_color(mut self, color: impl Into<String>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// What it is for.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_builder_sets_its_own_field_and_no_other() {
        let bare = Label::new("docs");

        assert_eq!(
            bare.clone().with_color("0075ca"),
            Label {
                color: Some("0075ca".to_string()),
                ..bare.clone()
            }
        );
        assert_eq!(
            bare.clone()
                .with_description("Improvements to documentation"),
            Label {
                description: Some("Improvements to documentation".to_string()),
                ..bare
            }
        );
    }
}
