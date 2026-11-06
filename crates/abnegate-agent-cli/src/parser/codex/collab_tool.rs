use serde::Deserialize;

/// One of the tools codex starts, talks to, waits on and closes the agents
/// it runs beside itself with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CollabTool {
    SpawnAgent,
    SendInput,
    Wait,
    CloseAgent,
}
