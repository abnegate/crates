use super::*;
use crate::pull_request::AutoMerge;
use crate::pull_request::EnabledAutoMerge;
use crate::pull_request::MergeMethod;
use crate::pull_request::MergeSettings;
use crate::pull_request::PullRequestAutomation;
use crate::pull_request::graphql_auto_merge_request::GraphQlAutoMergeRequest;
use crate::pull_request::graphql_automated_repository::GraphQlAutomatedRepository;
use crate::pull_request::graphql_error::GraphQlError;
use crate::pull_request::graphql_merge_settings::GraphQlMergeSettings;
use crate::pull_request::graphql_query::GraphQlQuery;
use crate::pull_request::service::graphql::FORBIDDEN;
use crate::pull_request::service::graphql::MODIFIED;
use crate::pull_request::service::graphql::NO_DATA;
use crate::pull_request::service::graphql::NOT_FOUND;
use crate::pull_request::service::graphql::RATE_LIMITED;
use crate::pull_request::service::graphql::messages;
use crate::pull_request::service::graphql::rejection;
use crate::pull_request::service::graphql::reported;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

/// Reads how a repository lets its pull requests merge.
const SETTINGS: &str = "query($owner:String!,$name:String!){\
repository(owner:$owner,name:$name){\
autoMergeAllowed squashMergeAllowed mergeCommitAllowed rebaseMergeAllowed}}";

/// Reads how a repository lets its pull requests merge, and whether one of
/// them is a draft, set to merge once it is ready, and what the token may
/// change about that.
const AUTOMATION: &str = "query($owner:String!,$name:String!,$number:Int!){\
repository(owner:$owner,name:$name){\
autoMergeAllowed squashMergeAllowed mergeCommitAllowed rebaseMergeAllowed \
pullRequest(number:$number){id isDraft \
autoMergeRequest{mergeMethod enabledAt enabledBy{login} commitHeadline commitBody} \
viewerCanEnableAutoMerge viewerCanDisableAutoMerge}}}";

/// Sets the pull request its `input` names to merge once it is ready.
const ENABLE_AUTO_MERGE: &str = "mutation($input:EnablePullRequestAutoMergeInput!){\
enablePullRequestAutoMerge(input:$input){clientMutationId}}";

/// Takes auto-merge off the pull request its `id` names.
const DISABLE_AUTO_MERGE: &str =
    "mutation($id:ID!){disablePullRequestAutoMerge(input:{pullRequestId:$id}){clientMutationId}}";

/// Turns the pull request its `id` names back into a draft.
const CONVERT_TO_DRAFT: &str =
    "mutation($id:ID!){convertPullRequestToDraft(input:{pullRequestId:$id}){clientMutationId}}";

/// Takes the pull request its `id` names out of draft.
const MARK_READY_FOR_REVIEW: &str = "mutation($id:ID!){\
markPullRequestReadyForReview(input:{pullRequestId:$id}){clientMutationId}}";

/// Where the answer to [`ENABLE_AUTO_MERGE`] holds what it did.
const ENABLED: &str = "enablePullRequestAutoMerge";

/// Where the answer to [`DISABLE_AUTO_MERGE`] holds what it did.
const DISABLED: &str = "disablePullRequestAutoMerge";

/// Where the answer to [`CONVERT_TO_DRAFT`] holds what it did.
const CONVERTED: &str = "convertPullRequestToDraft";

/// Where the answer to [`MARK_READY_FOR_REVIEW`] holds what it did.
const MARKED_READY: &str = "markPullRequestReadyForReview";

/// What GitHub says when the repository does not allow auto-merge.
const AUTO_MERGE_NOT_ALLOWED: &str = "auto merge is not allowed";

/// What GitHub says of a merge method the repository does not allow, as in
/// "Merge method squash merging is not allowed on this repository".
const MERGE_METHOD: &str = "merge method";

/// What GitHub says alongside [`MERGE_METHOD`] of one the repository does not
/// allow.
const NOT_ALLOWED: &str = "not allowed";

/// The merge states GitHub names when a pull request has nothing left to wait
/// for, as in "Pull request is in clean status": it merges cleanly, with or
/// without a pre-receive hook, and only checks that are not required have
/// failed, if any.
const MERGEABLE_NOW: &[&str] = &["clean status", "has_hooks status", "unstable status"];

/// What GitHub says of a pull request whose merge state keeps auto-merge from
/// being set, as in "Pull request is in dirty status".
const IN_STATUS: &str = "pull request is in";

impl PullRequestService {
    /// How a repository lets its pull requests merge: whether auto-merge is
    /// allowed, and with which methods.
    ///
    /// A repository GitHub cannot find, or will not show the token, is
    /// [`PullRequestError::NotFound`].
    pub async fn fetch_merge_settings(
        &self,
        repository: &Repository,
        token: &SecretValue,
    ) -> PullRequestResult<MergeSettings> {
        let answer: GraphQlQuery<GraphQlMergeSettings> = self
            .graphql(
                token,
                SETTINGS,
                &json!({ "owner": repository.owner(), "name": repository.name() }),
            )
            .await?;
        let found = answer.repository.ok_or(PullRequestError::NotFound)?;
        Ok(settings(&found))
    }

    /// Whether a pull request is a draft and set to merge once it is ready,
    /// what the token may change about that, and how its repository lets it
    /// merge, in one read.
    ///
    /// A repository or pull request GitHub cannot find, or will not show the
    /// token, is [`PullRequestError::NotFound`].
    pub async fn fetch_automation(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<PullRequestAutomation> {
        let repository = reference.repository();
        let answer: GraphQlQuery<GraphQlAutomatedRepository> = self
            .graphql(
                token,
                AUTOMATION,
                &json!({
                    "owner": repository.owner(),
                    "name": repository.name(),
                    "number": reference.number().get(),
                }),
            )
            .await?;
        let found = answer.repository.ok_or(PullRequestError::NotFound)?;
        let pull = found.pull_request.ok_or(PullRequestError::NotFound)?;
        Ok(PullRequestAutomation {
            node_id: pull.id,
            draft: pull.is_draft,
            auto_merge: pull.auto_merge_request.map(enabled),
            can_enable_auto_merge: pull.viewer_can_enable_auto_merge,
            can_disable_auto_merge: pull.viewer_can_disable_auto_merge,
            settings: settings(&found.settings),
        })
    }

    /// Set a pull request to merge as `request` says once everything its base
    /// requires is met.
    ///
    /// `node` is the pull request's GraphQL node id, as
    /// [`crate::pull_request::PullRequestDetail::node_id`] and
    /// [`PullRequestAutomation::node_id`] carry it; it travels as a variable,
    /// never inside the mutation.
    ///
    /// A repository that does not allow auto-merge is
    /// [`PullRequestError::AutoMergeNotAllowed`], and one that does not allow
    /// the method asked for is [`PullRequestError::MergeMethodNotAllowed`]. A
    /// pull request with nothing left to wait for, which can be merged now
    /// instead, is [`PullRequestError::AlreadyMergeable`], and one whose merge
    /// state otherwise keeps auto-merge from being set is
    /// [`PullRequestError::NotMergeable`], with GitHub's reason; one GitHub
    /// names as in unknown status has not had its merge state worked out yet,
    /// and may be set once it has. A head that moved from the one `request`
    /// insists on is [`PullRequestError::HeadMoved`]. A token that may not set
    /// it is [`PullRequestError::Forbidden`].
    pub async fn enable_auto_merge(
        &self,
        node: &str,
        token: &SecretValue,
        request: &AutoMerge,
    ) -> PullRequestResult<()> {
        let answer = self
            .answer(
                token,
                ENABLE_AUTO_MERGE,
                &json!({ "input": input(node, request) }),
            )
            .await?;
        if !answer.errors.is_empty() {
            return Err(auto_merge_refusal(&answer.errors));
        }
        changed(answer.data.as_ref(), ENABLED)
    }

    /// Take auto-merge off a pull request, so it waits to be merged by hand.
    ///
    /// `node` is the pull request's GraphQL node id; it travels as a variable,
    /// never inside the mutation.
    pub async fn disable_auto_merge(
        &self,
        node: &str,
        token: &SecretValue,
    ) -> PullRequestResult<()> {
        self.change(node, token, DISABLE_AUTO_MERGE, DISABLED).await
    }

    /// Turn a pull request back into a draft, which cannot be merged until it
    /// is marked ready for review again.
    ///
    /// `node` is the pull request's GraphQL node id; it travels as a variable,
    /// never inside the mutation.
    pub async fn convert_to_draft(&self, node: &str, token: &SecretValue) -> PullRequestResult<()> {
        self.change(node, token, CONVERT_TO_DRAFT, CONVERTED).await
    }

    /// Take a pull request out of draft, so it can be reviewed and merged.
    ///
    /// `node` is the pull request's GraphQL node id; it travels as a variable,
    /// never inside the mutation.
    pub async fn mark_ready_for_review(
        &self,
        node: &str,
        token: &SecretValue,
    ) -> PullRequestResult<()> {
        self.change(node, token, MARK_READY_FOR_REVIEW, MARKED_READY)
            .await
    }

    /// One mutation of the pull request `node` names, which takes nothing
    /// else and answers with what it did under `field`.
    async fn change(
        &self,
        node: &str,
        token: &SecretValue,
        mutation: &'static str,
        field: &str,
    ) -> PullRequestResult<()> {
        let data: Value = self
            .graphql(token, mutation, &json!({ "id": node }))
            .await?;
        changed(Some(&data), field)
    }
}

/// Whether a mutation's answer says what it did under `field`: an answer
/// that leaves it out or null did nothing this crate can vouch for.
fn changed(data: Option<&Value>, field: &str) -> PullRequestResult<()> {
    match data
        .and_then(|data| data.get(field))
        .is_some_and(Value::is_object)
    {
        true => Ok(()),
        false => Err(PullRequestError::GitHubApi(NO_DATA.to_string())),
    }
}

/// The input to the mutation that sets `node` to merge as `request` says,
/// naming only what `request` sets.
fn input(node: &str, request: &AutoMerge) -> Value {
    let mut input = Map::new();
    input.insert("pullRequestId".to_string(), json!(node));
    input.insert("mergeMethod".to_string(), json!(request.method.graphql()));
    if let Some(head) = &request.head {
        input.insert("expectedHeadOid".to_string(), json!(head.as_str()));
    }
    if let Some(title) = &request.title {
        input.insert("commitHeadline".to_string(), json!(title));
    }
    if let Some(message) = &request.message {
        input.insert("commitBody".to_string(), json!(message));
    }
    Value::Object(input)
}

/// How a repository GitHub answered for lets its pull requests merge.
fn settings(found: &GraphQlMergeSettings) -> MergeSettings {
    let methods = [
        (MergeMethod::Squash, found.squash_merge_allowed),
        (MergeMethod::Merge, found.merge_commit_allowed),
        (MergeMethod::Rebase, found.rebase_merge_allowed),
    ]
    .into_iter()
    .filter_map(|(method, allowed)| allowed.then_some(method))
    .collect();
    MergeSettings::new(found.auto_merge_allowed, methods)
}

/// The auto-merge GitHub answered for, with an empty login where it names
/// nobody.
fn enabled(request: GraphQlAutoMergeRequest) -> EnabledAutoMerge {
    EnabledAutoMerge {
        method: request
            .merge_method
            .as_deref()
            .and_then(MergeMethod::from_graphql),
        enabled_by: request.enabled_by.and_then(|actor| actor.login),
        enabled_at: request.enabled_at,
        title: request.commit_headline,
        message: request.commit_body,
    }
}

/// What the errors GitHub refused to set auto-merge with mean: a spent rate
/// limit is one; something missing or forbidden is that, whatever GitHub's
/// words say; a repository that does not allow auto-merge, or the method
/// asked for, says so; a head or base that GitHub says was modified moved; a
/// pull request with nothing left to wait for can be merged now; one in any
/// other merge state GitHub names is not mergeable as it stands; and anything
/// else is read as every other GraphQL refusal is.
fn auto_merge_refusal(errors: &[GraphQlError]) -> PullRequestError {
    if reported(errors, RATE_LIMITED) {
        return PullRequestError::RateLimited;
    }
    if reported(errors, NOT_FOUND) || reported(errors, FORBIDDEN) {
        return rejection(errors);
    }
    let said: Vec<String> = errors
        .iter()
        .map(|error| error.message.to_ascii_lowercase())
        .collect();
    let any = |test: &dyn Fn(&str) -> bool| said.iter().any(|message| test(message));
    if any(&|message| message.contains(AUTO_MERGE_NOT_ALLOWED)) {
        return PullRequestError::AutoMergeNotAllowed;
    }
    if any(&|message| message.contains(MERGE_METHOD) && message.contains(NOT_ALLOWED)) {
        return PullRequestError::MergeMethodNotAllowed(messages(errors));
    }
    if any(&|message| message.contains(MODIFIED)) {
        return PullRequestError::HeadMoved;
    }
    if any(&|message| MERGEABLE_NOW.iter().any(|state| message.contains(state))) {
        return PullRequestError::AlreadyMergeable;
    }
    if any(&|message| message.contains(IN_STATUS)) {
        return PullRequestError::NotMergeable(messages(errors));
    }
    rejection(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    /// The GraphQL node id of pull request 7.
    const NODE: &str = "PR_kwDOAcme7";

    /// Where every call here is posted.
    const GRAPHQL_PATH: &str = "/graphql";

    /// An answer carrying `data` and no errors.
    fn carrying(data: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "data": data }))
    }

    /// An answer carrying `errors` and no data.
    fn refused(errors: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "data": null, "errors": errors }))
    }

    /// A server that answers `query` with `variables`, sent with the token,
    /// exactly once, with `response`.
    async fn expecting(query: &str, variables: Value, response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(GRAPHQL_PATH))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({ "query": query, "variables": variables })))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    /// A server that answers every GraphQL call with `response`.
    async fn answering(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(GRAPHQL_PATH))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    /// A repository that allows auto-merge, squash and rebase, holding
    /// `pull_request`.
    fn repository_with(pull_request: Value) -> Value {
        json!({ "repository": {
            "autoMergeAllowed": true,
            "squashMergeAllowed": true,
            "mergeCommitAllowed": false,
            "rebaseMergeAllowed": true,
            "pullRequest": pull_request,
        } })
    }

    /// The variables that name pull request 7.
    fn seven_variables() -> Value {
        json!({ "owner": "acme", "name": "project", "number": 7 })
    }

    #[tokio::test]
    async fn merge_settings_are_read_for_the_repository_named_in_variables() {
        let server = expecting(
            SETTINGS,
            json!({ "owner": "acme", "name": "project" }),
            carrying(json!({ "repository": {
                "autoMergeAllowed": true,
                "squashMergeAllowed": true,
                "mergeCommitAllowed": false,
                "rebaseMergeAllowed": true,
            } })),
        )
        .await;
        let service = stand_in(&server).await;

        let settings = service
            .fetch_merge_settings(&project(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            settings,
            MergeSettings::new(true, vec![MergeMethod::Squash, MergeMethod::Rebase])
        );
        assert!(settings.allows(MergeMethod::Squash));
        assert!(!settings.allows(MergeMethod::Merge));
    }

    #[tokio::test]
    async fn merge_settings_of_a_repository_github_will_not_show_are_not_found() {
        for response in [
            carrying(json!({ "repository": null })),
            refused(
                json!([{ "type": "NOT_FOUND", "message": "Could not resolve to a Repository" }]),
            ),
        ] {
            let server = answering(response).await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_merge_settings(&project(&service), &token())
                .await
                .unwrap_err();

            assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn automation_reads_draft_auto_merge_and_what_the_token_may_change() {
        let server = expecting(
            AUTOMATION,
            seven_variables(),
            carrying(repository_with(json!({
                "id": NODE,
                "isDraft": false,
                "autoMergeRequest": {
                    "mergeMethod": "SQUASH",
                    "enabledAt": "2026-11-05T09:30:00Z",
                    "enabledBy": { "login": "ada" },
                    "commitHeadline": "feat: basket totals",
                    "commitBody": "Adds totals.",
                },
                "viewerCanEnableAutoMerge": false,
                "viewerCanDisableAutoMerge": true,
            }))),
        )
        .await;
        let service = stand_in(&server).await;

        let automation = service
            .fetch_automation(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            automation,
            PullRequestAutomation::new(
                NODE,
                MergeSettings::new(true, vec![MergeMethod::Squash, MergeMethod::Rebase]),
            )
            .with_auto_merge(
                EnabledAutoMerge::new(Some(MergeMethod::Squash))
                    .with_enabled_by("ada")
                    .with_enabled_at("2026-11-05T09:30:00Z")
                    .with_title("feat: basket totals")
                    .with_message("Adds totals."),
            )
            .with_can_disable_auto_merge(true)
        );
    }

    #[tokio::test]
    async fn a_draft_without_auto_merge_reads_as_one() {
        let server = answering(carrying(repository_with(json!({
            "id": NODE,
            "isDraft": true,
            "autoMergeRequest": null,
            "viewerCanEnableAutoMerge": true,
            "viewerCanDisableAutoMerge": false,
        }))))
        .await;
        let service = stand_in(&server).await;

        let automation = service
            .fetch_automation(&seven(&service), &token())
            .await
            .unwrap();

        assert!(automation.draft);
        assert_eq!(automation.auto_merge, None);
        assert!(automation.can_enable_auto_merge);
        assert!(!automation.can_disable_auto_merge);
    }

    #[tokio::test]
    async fn auto_merge_with_a_method_this_crate_does_not_know_still_reads_as_set() {
        let server = answering(carrying(repository_with(json!({
            "id": NODE,
            "isDraft": false,
            "autoMergeRequest": { "mergeMethod": "FAST_FORWARD", "enabledBy": null },
            "viewerCanEnableAutoMerge": false,
            "viewerCanDisableAutoMerge": true,
        }))))
        .await;
        let service = stand_in(&server).await;

        let automation = service
            .fetch_automation(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(automation.auto_merge, Some(EnabledAutoMerge::new(None)));
    }

    #[tokio::test]
    async fn automation_of_a_pull_request_github_will_not_show_is_not_found() {
        for response in [
            carrying(json!({ "repository": null })),
            carrying(repository_with(Value::Null)),
            refused(
                json!([{ "type": "NOT_FOUND", "message": "Could not resolve to a PullRequest" }]),
            ),
        ] {
            let server = answering(response).await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_automation(&seven(&service), &token())
                .await
                .unwrap_err();

            assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
        }
    }

    /// GitHub's answer to auto-merge it set.
    fn enabled_answer() -> ResponseTemplate {
        carrying(json!({ "enablePullRequestAutoMerge": { "clientMutationId": null } }))
    }

    #[tokio::test]
    async fn auto_merge_names_only_the_pull_request_and_method_unless_asked_for_more() {
        let server = expecting(
            ENABLE_AUTO_MERGE,
            json!({ "input": { "pullRequestId": NODE, "mergeMethod": "REBASE" } }),
            enabled_answer(),
        )
        .await;
        let service = stand_in(&server).await;

        service
            .enable_auto_merge(NODE, &token(), &AutoMerge::new(MergeMethod::Rebase))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn auto_merge_carries_the_head_it_insists_on_and_the_commit_it_makes() {
        let server = expecting(
            ENABLE_AUTO_MERGE,
            json!({ "input": {
                "pullRequestId": NODE,
                "mergeMethod": "SQUASH",
                "expectedHeadOid": commit('a').as_str(),
                "commitHeadline": "feat: basket totals",
                "commitBody": "Adds totals.",
            } }),
            enabled_answer(),
        )
        .await;
        let service = stand_in(&server).await;

        service
            .enable_auto_merge(
                NODE,
                &token(),
                &AutoMerge::new(MergeMethod::Squash)
                    .with_head(commit('a'))
                    .with_title("feat: basket totals")
                    .with_message("Adds totals."),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_node_id_cannot_rewrite_the_auto_merge_mutation() {
        let node = "\"}){ injected }";
        let server = expecting(
            ENABLE_AUTO_MERGE,
            json!({ "input": { "pullRequestId": node, "mergeMethod": "MERGE" } }),
            enabled_answer(),
        )
        .await;
        let service = stand_in(&server).await;

        service
            .enable_auto_merge(node, &token(), &AutoMerge::new(MergeMethod::Merge))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_refusal_to_set_auto_merge_is_reported_as_what_it_is() {
        for (response, expected) in [
            (
                refused(json!([{
                    "type": "UNPROCESSABLE",
                    "message": "Auto merge is not allowed for this repository",
                }])),
                "AutoMergeNotAllowed",
            ),
            (
                refused(json!([{
                    "message": "Merge method merge commits are not allowed on this repository",
                }])),
                r#"MergeMethodNotAllowed("Merge method merge commits are not allowed on this repository")"#,
            ),
            (
                refused(json!([{ "message": "Pull request Pull request is in clean status" }])),
                "AlreadyMergeable",
            ),
            (
                refused(json!([{ "message": "Pull request Pull request is in has_hooks status" }])),
                "AlreadyMergeable",
            ),
            (
                refused(json!([{ "message": "Pull request Pull request is in unstable status" }])),
                "AlreadyMergeable",
            ),
            (
                refused(json!([{ "message": "Pull request Pull request is in dirty status" }])),
                r#"NotMergeable("Pull request Pull request is in dirty status")"#,
            ),
            (
                refused(json!([{ "message": "Pull request Pull request is in unknown status" }])),
                r#"NotMergeable("Pull request Pull request is in unknown status")"#,
            ),
            (
                refused(json!([{
                    "message": "Head branch was modified. Review and try the merge again.",
                }])),
                "HeadMoved",
            ),
            (
                refused(json!([{
                    "type": "FORBIDDEN",
                    "message": "Resource not accessible by integration",
                }])),
                "Forbidden",
            ),
            (
                refused(json!([{ "type": "NOT_FOUND", "message": "Could not resolve to a node" }])),
                "NotFound",
            ),
            (
                refused(json!([
                    { "message": "Auto merge is not allowed for this repository" },
                    { "type": "RATE_LIMITED", "message": "API rate limit exceeded" },
                ])),
                "RateLimited",
            ),
            (
                refused(json!([{ "message": "Something went wrong" }])),
                r#"GitHubApi("GitHub's GraphQL API refused: Something went wrong")"#,
            ),
            (
                ResponseTemplate::new(200).set_body_json(json!({ "data": null })),
                r#"GitHubApi("GitHub's GraphQL API answered with no data")"#,
            ),
            (ResponseTemplate::new(401), "AuthenticationFailed"),
            (
                ResponseTemplate::new(403).insert_header("retry-after", "60"),
                "RateLimited",
            ),
        ] {
            let server = answering(response).await;
            let service = stand_in(&server).await;

            let failure = service
                .enable_auto_merge(NODE, &token(), &AutoMerge::new(MergeMethod::Squash))
                .await
                .unwrap_err();

            assert_eq!(format!("{failure:?}"), expected);
        }
    }

    #[tokio::test]
    async fn auto_merge_draft_and_ready_each_name_the_pull_request_as_a_variable() {
        for (mutation, answered) in [
            (DISABLE_AUTO_MERGE, "disablePullRequestAutoMerge"),
            (CONVERT_TO_DRAFT, "convertPullRequestToDraft"),
            (MARK_READY_FOR_REVIEW, "markPullRequestReadyForReview"),
        ] {
            let server = expecting(
                mutation,
                json!({ "id": NODE }),
                carrying(json!({ answered: { "clientMutationId": null } })),
            )
            .await;
            let service = stand_in(&server).await;

            let changed = match mutation {
                DISABLE_AUTO_MERGE => service.disable_auto_merge(NODE, &token()).await,
                CONVERT_TO_DRAFT => service.convert_to_draft(NODE, &token()).await,
                _ => service.mark_ready_for_review(NODE, &token()).await,
            };

            changed.unwrap_or_else(|failure| panic!("{answered}: {failure:?}"));
        }
    }

    #[tokio::test]
    async fn a_refusal_to_change_a_draft_or_auto_merge_is_reported_as_what_it_is() {
        for (response, expected) in [
            (
                refused(json!([{
                    "type": "FORBIDDEN",
                    "message": "Resource not accessible by integration",
                }])),
                "Forbidden",
            ),
            (
                refused(json!([{ "type": "NOT_FOUND", "message": "Could not resolve to a node" }])),
                "NotFound",
            ),
            (
                refused(json!([{ "message": "Pull request is closed" }])),
                r#"GitHubApi("GitHub's GraphQL API refused: Pull request is closed")"#,
            ),
            (ResponseTemplate::new(401), "AuthenticationFailed"),
        ] {
            let server = answering(response).await;
            let service = stand_in(&server).await;

            for failure in [
                service.disable_auto_merge(NODE, &token()).await,
                service.convert_to_draft(NODE, &token()).await,
                service.mark_ready_for_review(NODE, &token()).await,
            ]
            .map(Result::unwrap_err)
            {
                assert_eq!(format!("{failure:?}"), expected);
            }
        }
    }

    #[tokio::test]
    async fn a_change_whose_answer_says_nothing_of_it_is_not_a_change() {
        for data in [
            Value::Null,
            json!({}),
            json!({ ENABLED: null, DISABLED: null, CONVERTED: null, MARKED_READY: null }),
        ] {
            let server =
                answering(ResponseTemplate::new(200).set_body_json(json!({ "data": data }))).await;
            let service = stand_in(&server).await;

            for failure in [
                service
                    .enable_auto_merge(NODE, &token(), &AutoMerge::new(MergeMethod::Squash))
                    .await,
                service.disable_auto_merge(NODE, &token()).await,
                service.convert_to_draft(NODE, &token()).await,
                service.mark_ready_for_review(NODE, &token()).await,
            ]
            .map(Result::unwrap_err)
            {
                assert_eq!(
                    format!("{failure:?}"),
                    format!("GitHubApi({NO_DATA:?})"),
                    "{data}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_missing_or_forbidden_pull_request_is_that_whatever_its_status_says() {
        for (kind, expected) in [("FORBIDDEN", "Forbidden"), ("NOT_FOUND", "NotFound")] {
            for message in [
                "Pull request Pull request is in clean status",
                "Pull request Pull request is in dirty status",
            ] {
                let server =
                    answering(refused(json!([{ "type": kind, "message": message }]))).await;
                let service = stand_in(&server).await;

                let failure = service
                    .enable_auto_merge(NODE, &token(), &AutoMerge::new(MergeMethod::Squash))
                    .await
                    .unwrap_err();

                assert_eq!(format!("{failure:?}"), expected, "{kind}: {message}");
            }
        }
    }

    #[tokio::test]
    async fn auto_merge_and_drafts_are_posted_under_api_on_an_enterprise_origin() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/graphql"))
            .respond_with(carrying(json!({
                ENABLED: { "clientMutationId": null },
                DISABLED: { "clientMutationId": null },
                CONVERTED: { "clientMutationId": null },
                MARKED_READY: { "clientMutationId": null },
            })))
            .expect(4)
            .mount(&server)
            .await;
        let service =
            PullRequestService::standing_in_for("github.com", &format!("{}/api/v3", server.uri()))
                .unwrap();

        service
            .enable_auto_merge(NODE, &token(), &AutoMerge::new(MergeMethod::Squash))
            .await
            .unwrap();
        service.disable_auto_merge(NODE, &token()).await.unwrap();
        service.convert_to_draft(NODE, &token()).await.unwrap();
        service.mark_ready_for_review(NODE, &token()).await.unwrap();
    }

    #[tokio::test]
    async fn a_refusal_to_set_auto_merge_never_carries_the_token() {
        let server = answering(refused(json!([{
            "message": concat!("Pull request is in dirty status for ghp_", "0123456789abcdefghij"),
        }])))
        .await;
        let service = stand_in(&server).await;

        let failure = service
            .enable_auto_merge(NODE, &token(), &AutoMerge::new(MergeMethod::Squash))
            .await
            .unwrap_err();

        assert_eq!(
            format!("{failure:?}"),
            r#"NotMergeable("Pull request is in dirty status for [REDACTED]")"#
        );
    }
}
