#![deny(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Sandboxed command execution with streaming output.
//!
//! A [`CommandExecutor`] spawns a process in its own session, streams stdout and
//! stderr back as they arrive, and enforces a timeout and an output ceiling.
//! A run is its process group: whether the process exits, times out or is
//! cancelled, whatever is left of its group is killed before the run is
//! reported, so nothing it started outlives it. A [`JobRegistry`] tracks
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
//! than running unconfined. What a job gets beyond filesystem and network
//! confinement depends on the backend -- see
//! [`Backend::enforces_single_process`] and [`Backend::enforces_execute_roots`]
//! -- and the handshake advertises only what the host enforces.
//!
//! [`InboundMessage`], [`OutboundMessage`] and [`NdjsonCodec`] carry the same
//! work over a pipe as newline-delimited JSON, so an executor can run as a
//! separate process driven by whatever started it.
//!
//! ```
//! use std::time::Duration;
//!
//! use abnegate_exec::CommandExecutor;
//! use abnegate_exec::InboundMessage;
//! use abnegate_exec::OutboundMessage;
//! use abnegate_exec::RunStart;
//! use tokio::sync::mpsc;
//!
//! # async fn run() -> Result<(), abnegate_exec::ExecutorError> {
//! let (sender, mut receiver) = mpsc::channel(64);
//! CommandExecutor::new()
//!     .spawn(
//!         &InboundMessage::RunStart(
//!             RunStart::new("greet", std::env::temp_dir(), "echo")
//!                 .with_arguments(["hello"])
//!                 .with_timeout(Duration::from_secs(5)),
//!         ),
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
//! sees only the names in [`DEFAULT_ENVIRONMENT`], with the executor's values,
//! and [`RunStart::environment`] on top. No proxy variable is among them; a
//! command reaches a proxy through [`Proxy`], or through a name the executor
//! [allows](EnvironmentPolicy::allow). [`ExecutorConfig::environment`] takes
//! any [`EnvironmentPolicy`]: more names, variables of its own, or, for an
//! executor that holds nothing a command must not read,
//! [`EnvironmentPolicy::inherit`].
//!
//! # Proxy routing
//!
//! [`Proxy::from_environment`] reads [`PROXY_URL_VARIABLE`] and, when it is
//! set, overlays the standard proxy variables onto every unconfined command
//! after its own environment, so a tool cannot accidentally route around it.
//! Only loopback bypasses the proxy unless [`PROXY_BYPASS_VARIABLE`] names
//! other hosts. Confined commands reach no network at all and are unaffected.
//!
//! # Platform support
//!
//! Unix only. Confinement additionally needs macOS or Linux; see
//! [`HOST_BACKEND`].

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_vendor = "apple"
)))]
compile_error!(
    "abnegate-exec watches Unix process groups and supports Linux, Android, FreeBSD and Apple hosts only; confinement additionally needs macOS or Linux"
);

pub mod error;
pub mod executor;
pub mod job;
pub mod protocol;
pub mod proxy;

pub use abnegate_secret::SecretValue;
pub use error::DaemonError;
pub use error::ExecutorError;
pub use error::JobError;
pub use error::ProtocolError;
pub use executor::Backend;
pub use executor::CommandExecutor;
pub use executor::Confinement;
pub use executor::ConfinementError;
pub use executor::ConfinementMode;
pub use executor::DEFAULT_ENVIRONMENT;
pub use executor::EnvironmentPolicy;
pub use executor::ExecutorConfig;
pub use executor::HOST_BACKEND;
pub use executor::Invocation;
pub use executor::JobHandle;
pub use job::JobRegistry;
pub use job::JobState;
pub use protocol::Capability;
pub use protocol::ConfinementRequest;
pub use protocol::ErrorCode;
pub use protocol::Hello;
pub use protocol::InboundMessage;
pub use protocol::LogLevel;
pub use protocol::NdjsonCodec;
pub use protocol::OutboundMessage;
pub use protocol::PROTOCOL_VERSION;
pub use protocol::Ping;
pub use protocol::ProcessTreeRequest;
pub use protocol::RunCancel;
pub use protocol::RunStart;
pub use protocol::RunStdin;
pub use proxy::DEFAULT_BYPASS;
pub use proxy::PROXY_BYPASS_VARIABLE;
pub use proxy::PROXY_URL_VARIABLE;
pub use proxy::Proxy;
