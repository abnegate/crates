//! Heuristic classification of an error or skip reason.

/// Heuristic classification of an error or skip reason.
///
/// [`classify`](Self::classify) is the keyword match a host can run without an
/// embedder. [`REFERENCES`](Self::REFERENCES) are the descriptions a host
/// embeds when it wants [`Category::nearest`](crate::Category::nearest) instead.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorClass {
    /// A deadline, timeout, or hung connection.
    Timeout,
    /// Permission, authentication, or authorization.
    Permission,
    /// Syntax or parse failure.
    Syntax,
    /// A test run that did not pass.
    TestFailure,
    /// A build or compilation failure.
    BuildFailure,
    /// A missing file, module, or dependency.
    NotFound,
    /// Conflicting changes.
    Conflict,
    /// A version or package mismatch.
    Dependency,
    /// Nothing else matched.
    Unknown,
}

impl ErrorClass {
    /// Descriptions a host embeds to classify by cosine rather than keywords.
    pub const REFERENCES: &'static [(&'static str, &'static str)] = &[
        (
            "timeout",
            "network timeout, connection timed out, request deadline exceeded, slow response",
        ),
        (
            "permission",
            "permission denied, access denied, authentication failed, authorization error, forbidden",
        ),
        (
            "syntax",
            "syntax error, parse error, unexpected token, invalid syntax, malformed input",
        ),
        (
            "test_failure",
            "test failed, test failure, assertion error, test case not passing, spec failure",
        ),
        (
            "build_failure",
            "build failed, compilation error, link error, build process failure",
        ),
        (
            "not_found",
            "not found, missing file, missing module, missing dependency, module not found",
        ),
        (
            "conflict",
            "merge conflict, git conflict, conflicting changes, conflict resolution needed",
        ),
        (
            "dependency",
            "dependency version mismatch, incompatible dependency, package version conflict",
        ),
    ];

    /// A stable label for storage.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Permission => "permission",
            Self::Syntax => "syntax",
            Self::TestFailure => "test_failure",
            Self::BuildFailure => "build_failure",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Dependency => "dependency",
            Self::Unknown => "unknown",
        }
    }

    /// Keyword match against `error`. Unknown text is [`Unknown`](Self::Unknown).
    pub fn classify(error: &str) -> Self {
        let lower = error.to_ascii_lowercase();
        if lower.contains("timeout") || lower.contains("timed out") {
            Self::Timeout
        } else if lower.contains("permission") || lower.contains("access denied") {
            Self::Permission
        } else if lower.contains("syntax") || lower.contains("parse") {
            Self::Syntax
        } else if lower.contains("test") && lower.contains("fail") {
            Self::TestFailure
        } else if lower.contains("build") && lower.contains("fail") {
            Self::BuildFailure
        } else if lower.contains("not found") || lower.contains("missing") {
            Self::NotFound
        } else if lower.contains("conflict") {
            Self::Conflict
        } else if lower.contains("dependency") || lower.contains("incompatible") {
            Self::Dependency
        } else {
            Self::Unknown
        }
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_each_named_class() {
        assert_eq!(
            ErrorClass::classify("Connection timed out"),
            ErrorClass::Timeout
        );
        assert_eq!(
            ErrorClass::classify("Permission denied"),
            ErrorClass::Permission
        );
        assert_eq!(
            ErrorClass::classify("Syntax error on line 5"),
            ErrorClass::Syntax
        );
        assert_eq!(
            ErrorClass::classify("Tests failed"),
            ErrorClass::TestFailure
        );
        assert_eq!(
            ErrorClass::classify("Build failed"),
            ErrorClass::BuildFailure
        );
        assert_eq!(ErrorClass::classify("File not found"), ErrorClass::NotFound);
        assert_eq!(ErrorClass::classify("Merge conflict"), ErrorClass::Conflict);
        assert_eq!(
            ErrorClass::classify("incompatible dependency"),
            ErrorClass::Dependency
        );
        assert_eq!(
            ErrorClass::classify("Some random error"),
            ErrorClass::Unknown
        );
        assert_eq!(ErrorClass::classify(""), ErrorClass::Unknown);
        assert_eq!(
            ErrorClass::classify("Build failed due to timeout"),
            ErrorClass::Timeout
        );
    }

    #[test]
    fn labels_round_trip_through_as_str() {
        for class in [
            ErrorClass::Timeout,
            ErrorClass::Permission,
            ErrorClass::Syntax,
            ErrorClass::TestFailure,
            ErrorClass::BuildFailure,
            ErrorClass::NotFound,
            ErrorClass::Conflict,
            ErrorClass::Dependency,
            ErrorClass::Unknown,
        ] {
            assert!(!class.as_str().is_empty());
        }
        assert_eq!(ErrorClass::REFERENCES.len(), 8);
    }
}
