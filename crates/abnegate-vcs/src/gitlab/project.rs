use crate::parse_error::ParseError;
use std::fmt;

/// The segments a URL's path reads as itself or its parent rather than as a
/// name.
const DOT_SEGMENTS: &[&str] = &[".", ".."];

/// A GitLab project, named by its path, such as `group/sub/project`, or by
/// its numeric id, such as `123`.
///
/// It travels as one percent-encoded path segment, so a `/` in it is sent as
/// `%2F` and it can name no other endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Project(String);

impl Project {
    /// Accept `value` as it is, untrimmed, or fail with
    /// [`ParseError::Project`] when it is empty, `.` or `..`, or holds an
    /// ASCII control character.
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        let refused = value.is_empty()
            || DOT_SEGMENTS.contains(&value)
            || value.chars().any(|character| character.is_ascii_control());
        match refused {
            true => Err(ParseError::Project(value.to_string())),
            false => Ok(Self(value.to_string())),
        }
    }

    /// The project's path or id, as it was given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Project {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_or_a_numeric_id_is_a_project() {
        for name in ["myproject", "owner/project", "123", "group/sub/project"] {
            let project = Project::parse(name).expect(name);

            assert_eq!(project.as_str(), name);
            assert_eq!(project.to_string(), name);
        }
    }

    #[test]
    fn an_empty_or_dot_name_or_one_with_a_control_character_is_no_project() {
        for name in ["", "project\n", "project\0", "project\r", ".", ".."] {
            assert_eq!(
                Project::parse(name),
                Err(ParseError::Project(name.to_string())),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_refusal_names_what_arrived() {
        let refusal = Project::parse("..").unwrap_err().to_string();

        assert_eq!(refusal, "Invalid GitLab project: \"..\"");
    }
}
