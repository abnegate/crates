use base64::Engine;
use tokio::process::Command;

/// Authenticate a git network command without putting the token in the URL.
///
/// A credential in the remote URL reaches `.git/config`, the process table and
/// any error text that echoes the remote. The header is scoped to github.com so
/// a redirect elsewhere cannot carry it.
pub(crate) fn authenticate(command: &mut Command, token: &str) {
    let authorization =
        base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    command
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraHeader")
        .env(
            "GIT_CONFIG_VALUE_0",
            format!("Authorization: Basic {authorization}"),
        );
}
