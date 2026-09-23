use crate::repository_url::RepositoryUrl;
use abnegate_secret::SecretValue;
use base64::Engine;
use tokio::process::Command;

/// Authenticate a git network command without putting the token in the URL.
///
/// A credential in the remote URL reaches `.git/config`, the process table and
/// any error text that echoes the remote. The header is scoped to the one
/// repository the command was given, so a redirect, a submodule or a lazy
/// fetch that reaches anywhere else -- another repository on the same host
/// included -- cannot carry it.
pub(crate) fn authenticate(command: &mut Command, remote: &RepositoryUrl, token: &SecretValue) {
    let authorization = base64::engine::general_purpose::STANDARD
        .encode(format!("x-access-token:{}", token.expose()));
    command
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", format!("http.{remote}.extraHeader"))
        .env(
            "GIT_CONFIG_VALUE_0",
            format!("Authorization: Basic {authorization}"),
        );
}
