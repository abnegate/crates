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
use std::time::Duration;

use abnegate_exec::CommandExecutor;
use abnegate_exec::ExecutorError;
use abnegate_exec::InboundMessage;
use abnegate_exec::OutboundMessage;
use abnegate_exec::RunStart;
use tokio::sync::mpsc;

async fn greet() -> Result<(), ExecutorError> {
    let request = RunStart::new("greet", std::env::temp_dir(), "echo")
        .with_arguments(["hello"])
        .with_timeout(Duration::from_secs(5));
    let (sender, mut receiver) = mpsc::channel(64);
    CommandExecutor::new()
        .spawn(&InboundMessage::RunStart(request), sender)
        .await?;
    while let Some(message) = receiver.recv().await {
        println!("{message:?}");
        if matches!(message, OutboundMessage::RunExit { .. }) {
            break;
        }
    }
    Ok(())
}
```
