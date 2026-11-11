/// Why an [`McpHub`](super::McpHub) could not connect a server, as a kind a
/// host can act on without reading the failure's text.
///
/// It is taken from the typed error behind the failure and from the client's
/// own checks, never from the words of an error, so a server's reply or a
/// bound secret cannot change it. A forward proxy that answers in the
/// server's place is named as the server would be for that answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum McpFailure {
    /// The server is not [valid](super::McpServer::valid): it does not set
    /// exactly one of a command and a URL, or its `type` does not match the
    /// one it sets.
    Invalid,
    /// The server's transport is one this client does not speak:
    /// [`sse`](super::McpTransport::Sse).
    Unsupported,
    /// A `${VAR}` in the server's URL or headers names no
    /// [secret](super::McpServer::secrets) bound to it and has no default.
    Unresolved,
    /// The server's URL does not parse, or is neither `http` nor `https`.
    Address,
    /// The server's address is link-local or a cloud metadata service, as
    /// written or as every address its name resolved to. A name that
    /// resolves to no address at all is reported as this kind too.
    Metadata,
    /// One of the server's headers is not a valid header, is one the
    /// transport sets itself, or repeats another's name.
    Header,
    /// No HTTP client could be built to reach the server, as when the
    /// proxy's URL is not one.
    Client,
    /// The server did not complete its handshake and list its tools in
    /// time, or a request to it timed out.
    Timeout,
    /// The server answered with a redirect this client does not follow: one
    /// that leaves the origin first requested, changes the method, or comes
    /// after too many others.
    Redirect,
    /// The server, or a proxy in front of it, answered 401 or 403 with a
    /// `WWW-Authenticate` challenge the client could read. A 401 or 403
    /// without one, or with one that is not valid text, is
    /// [`Status`](Self::Status) with that code, unless it answered the third
    /// attempt, made after an HTTP/2 server refused the first two, which the
    /// client does not observe: that is [`Protocol`](Self::Protocol) with no
    /// status.
    Unauthorized,
    /// The server, or a proxy in front of it, answered with an HTTP status
    /// that is not a success and that no other kind names. An answer to the
    /// third attempt, made after an HTTP/2 server refused the first two, is
    /// not observed, so it can be [`Protocol`](Self::Protocol) with no
    /// status instead.
    Status,
    /// The server answered with something other than the MCP message the
    /// client was waiting for.
    Protocol,
    /// The server answered the handshake or the tool listing with a
    /// JSON-RPC error.
    Rejected,
    /// No connection to the server could be made.
    Unreachable,
    /// The server's command could not be started.
    Spawn,
    /// The server's answer was larger than the client reads.
    Oversized,
    /// The connection closed, or the server's process went away, before the
    /// handshake and the tool listing completed.
    Closed,
    /// A failure no other kind names.
    Other,
}
