#![deny(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Sandboxed command execution with streaming output.
//!
//! A [`CommandExecutor`] spawns a process in its own session, streams stdout and
//! stderr back as they arrive, enforces a timeout and an output ceiling, and
//! terminates the whole process tree on cancellation. A [`JobRegistry`] tracks
//! those runs by identifier so a caller can cancel one, or all of them, from
//! elsewhere.
//!
//! A run may ask to be confined. [`Confinement`] translates a command plus its
//! readable, writable and executable roots into an invocation of the host
//! sandbox — seatbelt on macOS, bubblewrap on Linux — with no network access at
//! all. Confinement is never assumed to work: [`Confinement::probe`] runs real
//! commands inside the sandbox and asserts that a denied file stays unreadable
//! and that a live local listener is never reached. A job that asked to be
//! confined fails to spawn on a host that cannot prove those properties, rather
//! than running unconfined.
//!
//! [`InboundMessage`], [`OutboundMessage`] and [`NdjsonCodec`] carry the same
//! work over a pipe as newline-delimited JSON, so an executor can run as a
//! separate process driven by whatever started it.
//!
//! ```
//! use abnegate_exec::{CommandExecutor, InboundMessage, OutboundMessage};
//! use std::collections::HashMap;
//! use tokio::sync::mpsc;
//!
//! # async fn run() -> Result<(), abnegate_exec::ExecutorError> {
//! let (sender, mut receiver) = mpsc::channel(64);
//! CommandExecutor::new()
//!     .spawn(
//!         &InboundMessage::RunStart {
//!             job_id: "greet".to_string(),
//!             workspace: std::env::temp_dir(),
//!             command: "echo".to_string(),
//!             args: vec!["hello".to_string()],
//!             env: HashMap::new(),
//!             working_dir: None,
//!             timeout_ms: Some(5_000),
//!             max_output_bytes: None,
//!             confinement: None,
//!         },
//!         sender,
//!     )
//!     .await?;
//!
//! while let Some(message) = receiver.recv().await {
//!     if let OutboundMessage::RunExit { exit_code, .. } = message {
//!         assert_eq!(exit_code, Some(0));
//!         break;
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Environment
//!
//! A command never inherits the executor's whole environment by default: it
//! sees only the names in [`DEFAULT_ENVIRONMENT_ALLOWLIST`], with the
//! executor's values, and the `RunStart.env` map on top. An executor that
//! holds nothing a command must not read can opt into
//! [`EnvironmentPolicy::Inherit`] through [`ExecutorConfig::environment`].
//!
//! # Proxy routing
//!
//! [`Proxy::from_env`] reads [`PROXY_URL_ENV`] and, when it is set, overlays the
//! standard proxy variables onto every unconfined command after its own
//! environment, so a tool cannot accidentally route around it. Only loopback
//! bypasses the proxy unless [`PROXY_BYPASS_ENV`] names other hosts. Confined
//! commands reach no network at all and are unaffected.
//!
//! # Platform support
//!
//! Unix only. Confinement additionally needs macOS or Linux; see
//! [`HOST_BACKEND`].

pub mod error;
pub mod executor;
pub mod job;
pub mod protocol;
pub mod proxy;

pub use error::{DaemonError, ExecutorError, JobError, ProtocolError};
pub use executor::{
    Backend, CommandExecutor, Confinement, ConfinementError, ConfinementMode,
    DEFAULT_ENVIRONMENT_ALLOWLIST, EnvironmentPolicy, ExecutorConfig, HOST_BACKEND, Invocation,
    JobHandle,
};
pub use job::{JobRegistry, JobState};
pub use protocol::{
    Capability, ConfinementRequest, ErrorCode, InboundMessage, LogLevel, NdjsonCodec,
    OutboundMessage, PROTOCOL_VERSION, ProcessTreeRequest,
};
pub use proxy::{DEFAULT_BYPASS, PROXY_BYPASS_ENV, PROXY_URL_ENV, Proxy};
