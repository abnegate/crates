/// Who signs an annotated tag: the name and email git records on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Tagger {
    /// The name recorded on the tag.
    pub name: String,
    /// The email recorded on the tag.
    pub email: String,
}

impl Tagger {
    /// The tagger `name`, reached at `email`.
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tagger_holds_what_it_was_made_with() {
        let tagger = Tagger::new("Release Bot", "release@example.com");

        assert_eq!(tagger.name, "Release Bot");
        assert_eq!(tagger.email, "release@example.com");
    }
}
