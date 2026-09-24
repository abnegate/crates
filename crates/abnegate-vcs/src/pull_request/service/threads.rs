use super::*;
use serde::de::IgnoredAny;

/// Marks the review thread its `id` names resolved.
const RESOLVE: &str =
    "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";

impl PullRequestService {
    /// Mark a review thread resolved.
    ///
    /// `thread` is the node id GitHub's GraphQL API names the thread by; it
    /// travels as a variable, never inside the mutation.
    pub async fn resolve_review_thread(
        &self,
        thread: &str,
        token: &SecretValue,
    ) -> PullRequestResult<()> {
        self.graphql::<IgnoredAny, _>(token, RESOLVE, &serde_json::json!({ "id": thread }))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    #[tokio::test]
    async fn resolving_a_thread_sends_its_id_as_a_variable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({
                "query": RESOLVE,
                "variables": { "id": "PRRT_1" },
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "resolveReviewThread": { "thread": { "isResolved": true } } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        service
            .resolve_review_thread("PRRT_1", &token())
            .await
            .unwrap();
    }
}
