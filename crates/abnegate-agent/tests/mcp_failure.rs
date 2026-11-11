#![cfg(feature = "mcp")]

use abnegate_agent::mcp::McpFailure;
use abnegate_agent::mcp::McpHub;
use abnegate_agent::mcp::McpOutcome;
use abnegate_agent::mcp::McpServer;
use abnegate_agent::tool::Proxy;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::any;

/// Written by the server, so a host must be able to say why the connection
/// failed without showing it.
const BODY: &str = "no MCP server lives here";

/// A host tells why a server failed from the outcome's kind and status
/// alone, never from its text, which can hold what the server sent.
#[tokio::test]
async fn a_host_tells_why_a_server_failed_without_reading_its_text() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(404).set_body_string(BODY))
        .mount(&server)
        .await;

    let outcome = McpHub::new()
        .reconnect(
            "docs",
            &McpServer::remote(format!("{}/mcp", server.uri())),
            &Proxy::default(),
        )
        .await;

    assert!(
        matches!(
            outcome,
            McpOutcome::Failed {
                failure: McpFailure::Status,
                status: Some(404),
                ..
            }
        ),
        "{outcome:?}"
    );
}
