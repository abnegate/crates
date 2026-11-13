/// Release notes GitHub wrote from the pull requests merged since the
/// previous release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GeneratedNotes {
    /// The title GitHub suggests for the release.
    pub name: String,
    /// The notes, in Markdown.
    pub body: String,
}

impl GeneratedNotes {
    /// Notes titled `name` that say `body`.
    pub fn new(name: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            body: body.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_hold_what_they_were_made_with() {
        let notes = GeneratedNotes::new("v1.2.0", "## What's Changed");

        assert_eq!(notes.name, "v1.2.0");
        assert_eq!(notes.body, "## What's Changed");
    }
}
