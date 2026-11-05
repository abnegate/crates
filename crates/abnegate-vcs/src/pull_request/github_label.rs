use crate::pull_request::Label;
use serde::Deserialize;

/// A label on an issue as the GitHub REST API returns it: an object naming
/// it, or on older answers its bare name.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum GitHubLabel {
    Named(String),
    Described {
        name: String,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
}

impl From<GitHubLabel> for Label {
    fn from(label: GitHubLabel) -> Self {
        match label {
            GitHubLabel::Named(name) => Self::new(name),
            GitHubLabel::Described {
                name,
                color,
                description,
            } => Self {
                name,
                color,
                description,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(label: serde_json::Value) -> Label {
        Label::from(serde_json::from_value::<GitHubLabel>(label).unwrap())
    }

    #[test]
    fn a_label_object_keeps_its_colour_and_description() {
        assert_eq!(
            read(json!({
                "id": 208_045_946,
                "name": "bug",
                "color": "d73a4a",
                "description": "Something isn't working",
                "default": true,
            })),
            Label::new("bug")
                .with_color("d73a4a")
                .with_description("Something isn't working")
        );
    }

    #[test]
    fn a_bare_label_name_is_a_label_with_nothing_else_said() {
        assert_eq!(read(json!("docs")), Label::new("docs"));
    }

    #[test]
    fn a_label_object_naming_only_itself_is_read() {
        assert_eq!(read(json!({ "name": "docs" })), Label::new("docs"));
        assert_eq!(
            read(json!({ "name": "docs", "color": null, "description": null })),
            Label::new("docs")
        );
    }

    #[test]
    fn a_label_without_a_name_is_refused() {
        for label in [json!({ "color": "d73a4a" }), json!(7), json!(null)] {
            assert!(
                serde_json::from_value::<GitHubLabel>(label.clone()).is_err(),
                "{label}"
            );
        }
    }
}
