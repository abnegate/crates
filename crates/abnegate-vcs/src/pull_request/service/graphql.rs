use super::*;
use crate::pull_request::graphql_error::GraphQlError;
use crate::pull_request::graphql_request::GraphQlRequest;
use crate::pull_request::graphql_response::GraphQlResponse;
use serde::Serialize;

/// The kind GitHub's GraphQL API names a spent rate limit with.
const RATE_LIMITED: &str = "RATE_LIMITED";

/// The kind GitHub's GraphQL API names something missing, or not visible to
/// the token, with.
const NOT_FOUND: &str = "NOT_FOUND";

/// The kind GitHub's GraphQL API names something the token may not do with.
const FORBIDDEN: &str = "FORBIDDEN";

/// What a refusal GitHub named no kind for says ahead of GitHub's own words.
const REFUSED: &str = "GitHub's GraphQL API refused";

/// What an answer that carried neither data nor errors is reported as.
const NO_DATA: &str = "GitHub's GraphQL API answered with no data";

/// What joins the messages of several GraphQL errors.
const SEPARATOR: &str = "; ";

impl PullRequestService {
    /// One GraphQL call, with any error GitHub reported turned into the
    /// refusal it names.
    pub(super) async fn graphql<T: DeserializeOwned, V: Serialize + ?Sized>(
        &self,
        token: &SecretValue,
        query: &'static str,
        variables: &V,
    ) -> PullRequestResult<T> {
        self.graphql_answer(token, query, variables)
            .await?
            .map_err(|errors| graphql_refusal(&errors))
    }

    /// One GraphQL call, with the errors GitHub reported handed back to a
    /// caller that reads them itself.
    ///
    /// A failed exchange is the outer error. A non-empty `errors` array is the
    /// inner one, and wins over whatever data came with it.
    pub(super) async fn graphql_answer<T: DeserializeOwned, V: Serialize + ?Sized>(
        &self,
        token: &SecretValue,
        query: &'static str,
        variables: &V,
    ) -> PullRequestResult<Result<T, Vec<GraphQlError>>> {
        let answer: GraphQlResponse = self
            .exchange(
                self.request(Method::POST, self.origin.graphql(), token, ACCEPT)
                    .json(&GraphQlRequest { query, variables }),
            )
            .await?;
        if !answer.errors.is_empty() {
            return Ok(Err(answer.errors));
        }
        let data = answer
            .data
            .ok_or_else(|| PullRequestError::GitHubApi(NO_DATA.to_string()))?;
        serde_json::from_value(data)
            .map(Ok)
            .map_err(|_| PullRequestError::GitHubApi(UNREADABLE.to_string()))
    }
}

/// The refusal a set of GraphQL errors names: a spent rate limit ahead of
/// anything missing, anything missing ahead of anything forbidden, and
/// GitHub's own words for the rest.
pub(super) fn graphql_refusal(errors: &[GraphQlError]) -> PullRequestError {
    let reported = |kind: &str| {
        errors
            .iter()
            .any(|error| error.kind.as_deref() == Some(kind))
    };
    if reported(RATE_LIMITED) {
        return PullRequestError::RateLimited;
    }
    if reported(NOT_FOUND) {
        return PullRequestError::NotFound;
    }
    if reported(FORBIDDEN) {
        return PullRequestError::Forbidden;
    }

    let messages = graphql_messages(errors);
    if messages.is_empty() {
        return PullRequestError::GitHubApi(REFUSED.to_string());
    }
    PullRequestError::GitHubApi(format!("{REFUSED}: {messages}"))
}

/// Every message in a set of GraphQL errors, joined, with no control
/// characters, and cut at 1 kB on a character boundary.
pub(super) fn graphql_messages(errors: &[GraphQlError]) -> String {
    let messages: String = errors
        .iter()
        .map(|error| error.message.as_str())
        .filter(|message| !message.is_empty())
        .collect::<Vec<&str>>()
        .join(SEPARATOR)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    bounded(&messages).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fmt::Debug;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// A query whose one value a caller chooses.
    const USER: &str = "query($login:String!){user(login:$login){name}}";

    async fn answering(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    async fn failure_for<T: DeserializeOwned + Debug>(
        response: ResponseTemplate,
    ) -> PullRequestError {
        let server = answering(response).await;
        stand_in(&server)
            .await
            .graphql::<T, _>(&token(), USER, &json!({ "login": "ada" }))
            .await
            .unwrap_err()
    }

    fn refused(errors: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "errors": errors }))
    }

    #[tokio::test]
    async fn a_graphql_query_travels_as_a_constant_with_its_values_as_variables() {
        let login = "\"){ injected }";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "query": USER,
                "variables": { "login": login },
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": { "user": { "name": "Ada" } } })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let answer: Value = service
            .graphql(&token(), USER, &json!({ "login": login }))
            .await
            .unwrap();

        assert_eq!(answer, json!({ "user": { "name": "Ada" } }));
        let received = server.received_requests().await.unwrap();
        let sent: Value = serde_json::from_slice(&received[0].body).unwrap();
        assert_eq!(
            sent["query"].as_str(),
            Some(USER),
            "the query is byte-identical whatever its variables hold"
        );
    }

    #[tokio::test]
    async fn graphql_is_posted_under_api_on_an_enterprise_origin() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": {} })))
            .expect(1)
            .mount(&server)
            .await;
        let service =
            PullRequestService::standing_in_for("github.com", &format!("{}/api/v3/", server.uri()))
                .unwrap();

        let answer: Value = service
            .graphql(&token(), USER, &json!({ "login": "ada" }))
            .await
            .unwrap();

        assert_eq!(answer, json!({}));
    }

    #[tokio::test]
    async fn a_graphql_refusal_is_reported_as_what_it_is() {
        for (response, expected) in [
            (
                refused(json!([{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }])),
                "RateLimited",
            ),
            (
                refused(json!([{ "type": "NOT_FOUND", "message": "Could not resolve to a node" }])),
                "NotFound",
            ),
            (
                refused(json!([{ "type": "FORBIDDEN", "message": "Resource not accessible" }])),
                "Forbidden",
            ),
            (
                refused(json!([
                    { "type": "FORBIDDEN", "message": "no" },
                    { "type": "NOT_FOUND", "message": "gone" },
                ])),
                "NotFound",
            ),
            (
                refused(json!([
                    { "type": "FORBIDDEN", "message": "no" },
                    { "type": "NOT_FOUND", "message": "gone" },
                    { "type": "RATE_LIMITED", "message": "slow down" },
                ])),
                "RateLimited",
            ),
            (
                refused(json!([{ "message": "Something went wrong" }])),
                r#"GitHubApi("GitHub's GraphQL API refused: Something went wrong")"#,
            ),
            (
                refused(json!([
                    { "message": "first" },
                    { "type": "UNPROCESSABLE", "message": "second" },
                ])),
                r#"GitHubApi("GitHub's GraphQL API refused: first; second")"#,
            ),
            (
                ResponseTemplate::new(200).set_body_string(
                    r#"{"data":null,"errors":[{"message":"Base branch was modified"}]}"#,
                ),
                r#"GitHubApi("GitHub's GraphQL API refused: Base branch was modified")"#,
            ),
            (
                ResponseTemplate::new(200).set_body_json(json!({
                    "data": { "user": { "name": "Ada" } },
                    "errors": [{ "type": "FORBIDDEN", "message": "partly hidden" }],
                })),
                "Forbidden",
            ),
            (
                ResponseTemplate::new(200).set_body_json(json!({
                    "data": { "user": { "name": "Ada" } },
                    "errors": [{ "message": "partly wrong" }],
                })),
                r#"GitHubApi("GitHub's GraphQL API refused: partly wrong")"#,
            ),
            (ResponseTemplate::new(401), "AuthenticationFailed"),
            (
                ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
                "RateLimited",
            ),
        ] {
            let failure = failure_for::<Value>(response).await;

            assert_eq!(format!("{failure:?}"), expected);
        }
    }

    #[tokio::test]
    async fn a_graphql_answer_with_no_data_is_an_error() {
        for body in [r#"{}"#, r#"{"data":null}"#, r#"{"data":null,"errors":[]}"#] {
            let failure =
                failure_for::<Value>(ResponseTemplate::new(200).set_body_string(body)).await;

            assert_eq!(
                format!("{failure:?}"),
                format!("GitHubApi({NO_DATA:?})"),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn a_graphql_answer_this_crate_cannot_read_names_no_part_of_it() {
        for body in [
            "RAW-SENTINEL",
            r#"{"errors":"RAW-SENTINEL"}"#,
            r#"{"data":{"user":"RAW-SENTINEL"}}"#,
        ] {
            let failure = failure_for::<BTreeMap<String, u64>>(
                ResponseTemplate::new(200).set_body_string(body),
            )
            .await;

            assert_eq!(
                format!("{failure:?}"),
                format!("GitHubApi({UNREADABLE:?})"),
                "{body}"
            );
            assert!(!failure.to_string().contains("SENTINEL"), "{failure}");
        }
    }

    #[tokio::test]
    async fn a_graphql_error_message_is_carried_only_so_far() {
        let long = "€".repeat(4096);
        let errors = [
            GraphQlError {
                message: long.clone(),
                kind: None,
            },
            GraphQlError {
                message: "after".to_string(),
                kind: None,
            },
        ];

        let messages = graphql_messages(&errors);

        assert_eq!(messages.len(), MAXIMUM_ERROR_BYTES - 1);
        assert!(messages.chars().all(|character| character == '€'));

        let failure = failure_for::<Value>(refused(
            json!([{ "message": long }, { "message": "after" }]),
        ))
        .await
        .to_string();
        assert!(
            failure.len() < MAXIMUM_ERROR_BYTES + 100,
            "{}",
            failure.len()
        );
        assert!(!failure.contains("after"));
    }

    #[test]
    fn a_graphql_error_message_cannot_forge_a_log_line() {
        let errors = [GraphQlError {
            message: "denied\nINFO forged\r\u{1b}[0m".to_string(),
            kind: None,
        }];

        assert_eq!(graphql_messages(&errors), "deniedINFO forged[0m");
    }
}
