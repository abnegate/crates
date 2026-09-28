use std::collections::BTreeMap;

use abnegate_secret::SecretValue;

/// A stdio server's value that refers to variables, as configured, with the
/// [secrets](crate::McpServer::secrets) bound to that server, which its
/// references read before anything the child is given or this process's
/// environment holds.
#[derive(Debug)]
pub(crate) struct Template {
    pub(crate) value: SecretValue,
    pub(crate) secrets: BTreeMap<String, SecretValue>,
}

impl Template {
    /// What the secret bound for `variable` holds, if one is.
    pub(crate) fn bound(&self, variable: &str) -> Option<String> {
        self.secrets
            .get(variable)
            .map(|secret| secret.expose().to_string())
    }
}
