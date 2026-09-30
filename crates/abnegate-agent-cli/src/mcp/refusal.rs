use std::fmt;

/// Why an enabled server never attaches to a CLI's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Not exactly one of `command` and `url`, one of them blank, a URL that
    /// resolves to blank text, or a transport that disagrees with it or that
    /// this crate does not attach over.
    Invalid,
    /// A name or tool name unsafe in `--allowedTools`.
    Unnameable,
    /// A reference in the URL or headers to a variable with no secret bound
    /// to the server and no default.
    Unbound,
    /// A URL or header value that still holds `${` once resolved, which the
    /// CLI could read as a reference of its own: it expands a header value a
    /// second time when it connects. It expands a URL only once, and one is
    /// refused alike rather than trusted to that.
    Expandable,
    /// A header name holding `${`, which the CLI never expands and would
    /// send as it is.
    HeaderName,
    /// A remote server over server-sent events, which Codex never reaches:
    /// it speaks streamable HTTP alone.
    ServerSentEvents,
    /// A remote URL that refers to a variable, which Codex could be given
    /// only on its command line, where what it resolves to would show.
    ReferringUrl,
    /// A stdio server's variable named other than as a shell identifier,
    /// which the shell Codex starts the server through could not set.
    VariableName,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => {
                "set exactly one of `command` and `url`, not blank once resolved, and a `type`, if any, of `stdio`, `http` or `sse` that matches it"
            }
            Self::Unnameable => {
                "its name and tool names may hold only letters, digits, `_` and `-`"
            }
            Self::Unbound => {
                "its URL or headers refer to a variable with no secret bound to this server and no default"
            }
            Self::Expandable => {
                "its URL or a header value still holds `${` once resolved, which the CLI could read as a reference"
            }
            Self::HeaderName => {
                "a header name holds `${`, which the CLI never expands and would send as it is"
            }
            Self::ServerSentEvents => {
                "it is reached over `sse`, and Codex reaches a remote server over streamable HTTP alone"
            }
            Self::ReferringUrl => {
                "its URL refers to a variable, and Codex takes a URL only on its command line, where what it resolves to would show"
            }
            Self::VariableName => {
                "a variable in its `env` is not named as a shell identifier, which Codex needs to hand its value over under a generated name"
            }
        })
    }
}
