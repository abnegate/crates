//! Codex's own device-code sign-in, as `codex login --device-auth` runs it:
//! codex prints a link and a one-time code, a person enters the code there,
//! and codex saves the login it is granted as `auth.json` in its
//! `CODEX_HOME`.
//!
//! [`DeviceLogin::start`] runs it in a private [`Staging`] directory and
//! returns the [`Device`] once codex has printed its [`Prompt`]. Its
//! [`outcome`](Device::outcome) resolves to the staging directory once codex
//! has saved a login there; [`cancel`](Device::cancel), or dropping it,
//! stops codex. [`Staging::promote`] moves the login into its home, and
//! [`Account`] reads whose login it is. Nothing is stored for the caller: it
//! may as well read the staged `auth.json` and keep it however it keeps
//! credentials.
//!
//! ```no_run
//! use std::collections::BTreeMap;
//! use std::path::Path;
//!
//! use abnegate_agent_cli::login::LoginError;
//! use abnegate_agent_cli::login::codex::DeviceLogin;
//!
//! async fn sign_in(root: &Path, home: &Path) -> Result<(), LoginError> {
//!     let environment = BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]);
//!     let login = DeviceLogin::new("codex").with_environment(environment);
//!     let mut device = login.start(root).await?;
//!     let prompt = device.prompt();
//!     println!("Open {} and enter {}", prompt.verification_url, prompt.user_code);
//!
//!     let staging = device.outcome().await?;
//!     if let Some(account) = staging.account() {
//!         println!("Signed in to ChatGPT account {}", account.id);
//!     }
//!     staging.promote(home)
//! }
//! ```

mod account;
mod device;
mod device_login;
mod limits;
mod plan;
mod prompt;
mod staging;
#[cfg(test)]
mod tests;

use std::fs;
use std::path::Path;

pub use crate::login::codex::account::Account;
pub use crate::login::codex::device::Device;
pub use crate::login::codex::device_login::DeviceLogin;
pub use crate::login::codex::limits::Limits;
pub use crate::login::codex::plan::Plan;
pub use crate::login::codex::prompt::Prompt;
pub use crate::login::codex::staging::Staging;

/// The file codex saves its login in, in its `CODEX_HOME`.
pub const CREDENTIALS: &str = "auth.json";

/// The variable naming the directory codex keeps its login in.
pub(crate) const HOME: &str = "CODEX_HOME";

const LOGIN: &[&str] = &["login", "--device-auth"];
const LOGOUT: &[&str] = &["logout"];

/// Whether `home` holds a codex login: its `auth.json` is a regular file,
/// not a link or a directory. Codex never checks the credentials in it until
/// a turn needs them.
pub fn signed_in(home: &Path) -> bool {
    fs::symlink_metadata(home.join(CREDENTIALS)).is_ok_and(|metadata| metadata.is_file())
}
