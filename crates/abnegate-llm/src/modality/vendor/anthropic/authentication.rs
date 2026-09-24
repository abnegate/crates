use abnegate_secret::SecretValue;

/// How a call proves who it is.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AnthropicAuthentication {
    /// An Anthropic API key, sent to the messages API as `x-api-key`.
    ApiKey(SecretValue),
    /// The Claude Code CLI's own credential, which the HTTP API does not
    /// accept: calls go through the CLI with it in `CLAUDE_CODE_OAUTH_TOKEN`.
    OAuthToken(SecretValue),
    /// No credentials of our own: the installed `claude` CLI holds them.
    ClaudeCli,
}
