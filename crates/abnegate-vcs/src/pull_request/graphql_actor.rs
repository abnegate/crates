use serde::Deserialize;

/// The GraphQL type GitHub names an App's account with, whose login its REST
/// API spells with a `[bot]` suffix.
const BOT: &str = "Bot";

/// The suffix GitHub's REST API spells an App's login with.
const BOT_SUFFIX: &str = "[bot]";

/// Apps whose login GitHub's REST API spells as another word altogether, by
/// the login its GraphQL API gives them.
const RENAMED: &[(&str, &str)] = &[("copilot-swe-agent", "Copilot")];

/// Whoever wrote something, as GitHub's GraphQL API names them.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlActor {
    #[serde(default)]
    pub(super) login: Option<String>,
    #[serde(default, rename = "__typename")]
    pub(super) kind: Option<String>,
}

impl GraphQlActor {
    /// The login as GitHub's REST API spells it: an App's with `[bot]` after
    /// it, as its GraphQL API leaves out, or, for the few Apps REST names
    /// another way, such as GitHub's Copilot coding agent, that name.
    pub(super) fn login_as_listed(self) -> Option<String> {
        let login = self.login?;
        if self.kind.as_deref() != Some(BOT) || login.ends_with(BOT_SUFFIX) {
            return Some(login);
        }
        match RENAMED.iter().find(|(graphql, _)| *graphql == login) {
            Some((_, rest)) => Some(rest.to_string()),
            None => Some(format!("{login}{BOT_SUFFIX}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_apps_login_reads_as_rest_spells_it_and_anyone_elses_as_it_is() {
        for (actor, expected) in [
            (
                json!({ "__typename": "Bot", "login": "review-app" }),
                Some("review-app[bot]"),
            ),
            (
                json!({ "__typename": "Bot", "login": "review-app[bot]" }),
                Some("review-app[bot]"),
            ),
            (
                json!({ "__typename": "User", "login": "octocat" }),
                Some("octocat"),
            ),
            (
                json!({ "__typename": "Mannequin", "login": "imported" }),
                Some("imported"),
            ),
            (json!({ "login": "octocat" }), Some("octocat")),
            (json!({ "__typename": "Bot", "login": null }), None),
        ] {
            let read: GraphQlActor = serde_json::from_value(actor.clone()).unwrap();

            assert_eq!(read.login_as_listed().as_deref(), expected, "{actor}");
        }
    }
}
