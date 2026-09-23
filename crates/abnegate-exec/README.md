# abnegate-exec

Sandboxed command execution with streaming output. A `CommandExecutor` spawns a
process in its own session, streams stdout and stderr back as they arrive, and
enforces a timeout and an output ceiling; whatever is left of the run's process
group is killed before the run is reported, so nothing it started outlives it. A
run may ask to be confined: `Confinement` runs it under seatbelt on macOS or
bubblewrap on Linux with no network access at all, and a confined job fails to
spawn on a host where `Confinement::probe` cannot prove the sandbox holds, rather
than running unconfined. A command sees only the allowlisted environment by
default, and `InboundMessage`, `OutboundMessage` and `NdjsonCodec` carry the same
work over a pipe as newline-delimited JSON. Unix only; confinement additionally
needs macOS or Linux.

## Features

None.

## Usage

```sh
cargo add abnegate-exec
cargo add tokio --features sync
```

```rust,no_run
use std::collections::HashMap;

use abnegate_exec::{CommandExecutor, ExecutorError, InboundMessage, OutboundMessage};
use tokio::sync::mpsc;

async fn greet() -> Result<(), ExecutorError> {
    let request = InboundMessage::RunStart {
        job_id: "greet".to_string(),
        workspace: std::env::temp_dir(),
        command: "echo".to_string(),
        args: vec!["hello".to_string()],
        env: HashMap::new(),
        working_dir: None,
        timeout_ms: Some(5_000),
        max_output_bytes: None,
        confinement: None,
    };
    let (sender, mut receiver) = mpsc::channel(64);
    CommandExecutor::new().spawn(&request, sender).await?;
    while let Some(message) = receiver.recv().await {
        println!("{message:?}");
        if matches!(message, OutboundMessage::RunExit { .. }) {
            break;
        }
    }
    Ok(())
}
```
