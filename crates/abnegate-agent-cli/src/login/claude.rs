//! Claude's own sign-in, as `claude setup-token` runs it: claude.com either
//! shows the code for pasting or sends the browser to a loopback listener
//! with it.
//!
//! [`Authorization::new`] draws a PKCE verifier and state and builds the
//! link to open. The code comes back pasted, read with [`Code::parse`], or in
//! the query of a loopback callback, read with [`Reply::read`]. A [`Client`]
//! exchanges it for [`Tokens`] and renews them later.
//!
//! ```no_run
//! use abnegate_agent_cli::login::LoginError;
//! use abnegate_agent_cli::login::claude::Authorization;
//! use abnegate_agent_cli::login::claude::Client;
//! use abnegate_agent_cli::login::claude::Code;
//! use abnegate_agent_cli::login::claude::Endpoints;
//! use abnegate_agent_cli::login::claude::Redirect;
//! use abnegate_agent_cli::login::claude::Scope;
//!
//! async fn sign_in(pasted: &str) -> Result<(), LoginError> {
//!     let endpoints = Endpoints::default();
//!     let authorization = Authorization::new(&endpoints, Scope::Inference, Redirect::Paste)?;
//!     println!("Open {}", authorization.url);
//!
//!     let code = Code::parse(&endpoints, pasted)?;
//!     let client = Client::new(endpoints)?;
//!     let tokens = client
//!         .exchange(
//!             &code,
//!             &authorization.verifier,
//!             authorization.scope,
//!             authorization.redirect,
//!         )
//!         .await?;
//!     println!("Signed in until {}", tokens.expires_at);
//!     Ok(())
//! }
//! ```

mod authorization;
mod client;
mod code;
mod endpoints;
mod flow;
mod plan;
mod redirect;
mod refusal;
mod reply;
mod scope;
mod tokens;

use std::time::Duration;

pub use crate::login::claude::authorization::Authorization;
pub use crate::login::claude::client::Client;
pub use crate::login::claude::code::Code;
pub use crate::login::claude::endpoints::Endpoints;
pub use crate::login::claude::flow::Flow;
pub use crate::login::claude::plan::Plan;
pub use crate::login::claude::redirect::CALLBACK_PATH;
pub use crate::login::claude::redirect::LOOPBACK_HOST;
pub use crate::login::claude::redirect::LOOPBACK_SCHEME;
pub use crate::login::claude::redirect::Redirect;
pub use crate::login::claude::refusal::Refusal;
pub use crate::login::claude::reply::Reply;
pub use crate::login::claude::scope::Scope;
pub use crate::login::claude::tokens::Tokens;

/// The OAuth client the claude CLI signs in as.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// Where the claude CLI sends the browser to approve a sign-in.
pub const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";

/// Where the claude CLI exchanges a code for tokens and renews them.
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// claude.com's own page, which shows the code to paste.
pub const REDIRECT_URL: &str = "https://platform.claude.com/oauth/code/callback";

/// How long an inference-only sign-in asks its access token to last, as
/// `claude setup-token` asks: a year.
pub const LIFETIME: Duration = Duration::from_secs(31_536_000);
