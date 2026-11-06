use serde::Deserialize;

/// One error GitHub's GraphQL API reported, the kind it named, if any, and
/// the path in the query it was reported at, if any.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlError {
    #[serde(default)]
    pub(super) message: String,
    #[serde(default, rename = "type")]
    pub(super) kind: Option<String>,
    #[serde(default)]
    pub(super) path: Option<Vec<serde_json::Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_graphql_error_with_a_null_or_no_path_reads_as_none() {
        for body in [
            json!({ "message": "gone", "type": "NOT_FOUND", "path": null }),
            json!({ "message": "gone", "type": "NOT_FOUND" }),
        ] {
            let error: GraphQlError = serde_json::from_value(body.clone()).unwrap();

            assert_eq!(error.path, None, "{body}");
            assert_eq!(error.kind.as_deref(), Some("NOT_FOUND"), "{body}");
        }
    }

    #[test]
    fn a_graphql_error_path_reads_as_its_segments() {
        let error: GraphQlError = serde_json::from_value(json!({
            "message": "gone",
            "path": ["repository", "pullRequest", 0],
        }))
        .unwrap();

        assert_eq!(
            error.path,
            Some(vec![json!("repository"), json!("pullRequest"), json!(0)])
        );
    }
}
