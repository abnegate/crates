mod exchange;
mod failure;
mod granted;
mod reason;
mod refresh;

use std::fmt;
use std::time::Duration;

use abnegate_http::metadata::MetadataResolver;
use abnegate_http::read_capped;
use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;
use chrono::Utc;
use reqwest::Proxy;
use reqwest::Url;
use reqwest::header::ACCEPT;
use reqwest::redirect::Policy;
use serde::Serialize;

use crate::login::LoginError;
use crate::login::claude::Code;
use crate::login::claude::Endpoints;
use crate::login::claude::Redirect;
use crate::login::claude::Scope;
use crate::login::claude::Tokens;
use crate::login::claude::client::exchange::Exchange;
use crate::login::claude::client::failure::Failure;
use crate::login::claude::client::granted::Granted;
use crate::login::claude::client::refresh::Refresh;
use crate::login::claude::endpoints::checked;

const TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("abnegate-agent-cli/", env!("CARGO_PKG_VERSION"));
const BODY_LIMIT: usize = 64 * 1024;
const JSON: &str = "application/json";
const AUTHORIZATION_CODE: &str = "authorization_code";
const REFRESH_TOKEN: &str = "refresh_token";
const UNREADABLE: &str = "Claude's token endpoint sent a grant that could not be read";
const UNRENEWABLE: &str = "This Claude sign-in has no refresh token";
const PROXY: &str = "proxy";
const BASIC_SEPARATOR: char = ':';
const USERNAME_REFUSED: &str = "a proxy username cannot hold a colon";

/// Exchanges a sign-in's code for [`Tokens`], and renews them, at the
/// [`Endpoints::token`] endpoint.
///
/// Its HTTP client is its own. It follows no redirect, so a redirect is a
/// [`LoginError::Rejected`], drops link-local and cloud metadata addresses a
/// name resolves to, gives up on a request after 30 seconds, reads at most
/// 64 KiB of an answer, and names itself `abnegate-agent-cli/<version>`. It
/// goes through no proxy, whatever this process's environment says, unless
/// one is given with [`Client::with_proxy`] or
/// [`Client::with_authenticated_proxy`].
///
/// Building one starts a connection pool and loads the TLS roots, so build
/// it once and clone it wherever it is needed: clones share the pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoints: Endpoints,
    proxy: Option<Url>,
    /// Kept only to scrub from what a failed request reports.
    proxy_password: Option<SecretValue>,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Client")
            .field("endpoints", &self.endpoints)
            .field("proxy", &self.proxy.as_ref().map(Url::as_str))
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client for `endpoints`, through no proxy.
    ///
    /// # Errors
    ///
    /// [`LoginError::Transport`] when the HTTP client cannot be built, such
    /// as when no TLS backend can start.
    pub fn new(endpoints: Endpoints) -> Result<Self, LoginError> {
        Ok(Self {
            http: http(None)?,
            endpoints,
            proxy: None,
            proxy_password: None,
        })
    }

    /// This client, sending every request through the HTTP proxy at
    /// `proxy`, such as `http://proxy.internal:3128`. The proxy is never read
    /// from the environment.
    ///
    /// The URL is checked as [`Endpoints`] checks theirs, so a loopback or
    /// LAN proxy is allowed, and a link-local or cloud metadata one, another
    /// scheme than `http` or `https`, or credentials in the URL are refused.
    /// Only the proxy's own name is resolved here; the proxy resolves the
    /// token endpoint's.
    ///
    /// # Errors
    ///
    /// [`LoginError::Endpoint`] naming the `proxy` when its URL is refused,
    /// and [`LoginError::Transport`] when the HTTP client cannot be built.
    pub fn with_proxy(self, proxy: &str) -> Result<Self, LoginError> {
        let proxy = checked(PROXY, proxy)?;
        Ok(Self {
            http: http(Some((&proxy, None)))?,
            proxy: Some(proxy),
            proxy_password: None,
            ..self
        })
    }

    /// This client, sending every request through the HTTP proxy at `proxy`
    /// and signing in to it as `username` with `password`.
    ///
    /// The URL is checked as [`Client::with_proxy`] checks it, so credentials
    /// still may not be written in it: they are given here so they stay out of
    /// [`Client::proxy`], `Debug` and every error.
    ///
    /// # Errors
    ///
    /// [`LoginError::Endpoint`] naming the `proxy` when its URL is refused or
    /// `username` holds a colon, which HTTP Basic cannot carry, and
    /// [`LoginError::Transport`] when the HTTP client cannot be built.
    pub fn with_authenticated_proxy(
        self,
        proxy: &str,
        username: &str,
        password: impl Into<SecretValue>,
    ) -> Result<Self, LoginError> {
        let proxy = checked(PROXY, proxy)?;
        if username.contains(BASIC_SEPARATOR) {
            return Err(LoginError::Endpoint {
                name: PROXY,
                reason: USERNAME_REFUSED.to_string(),
            });
        }
        let password = password.into();
        Ok(Self {
            http: http(Some((&proxy, Some((username, &password)))))?,
            proxy: Some(proxy),
            proxy_password: Some(password),
            ..self
        })
    }

    /// The proxy every request goes through, when one was given.
    pub fn proxy(&self) -> Option<&Url> {
        self.proxy.as_ref()
    }

    /// The endpoints this client signs in with.
    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// Exchange `code` for tokens, proving the sign-in with `verifier`.
    ///
    /// `scope` and `redirect` must be the ones the sign-in's
    /// [`Authorization`](crate::login::claude::Authorization) was built with:
    /// Claude grants a code only to the redirect it was issued for, and an
    /// inference sign-in asks for a token that lasts a
    /// [`LIFETIME`](crate::login::claude::LIFETIME). A grant that names no
    /// scope keeps the one asked for.
    ///
    /// # Errors
    ///
    /// [`LoginError::Rejected`] when the token endpoint answers with any
    /// status other than success, [`LoginError::Transport`] when it cannot
    /// be reached or its answer read, and [`LoginError::Malformed`] when its
    /// grant cannot be read. None repeats the code, the verifier or the
    /// state.
    pub async fn exchange(
        &self,
        code: &Code,
        verifier: &SecretValue,
        scope: Scope,
        redirect: Redirect,
    ) -> Result<Tokens, LoginError> {
        let redirect_uri = redirect.uri(&self.endpoints);
        let request = Exchange {
            grant_type: AUTHORIZATION_CODE,
            code: code.value.expose(),
            redirect_uri: &redirect_uri,
            client_id: self.endpoints.client_id(),
            code_verifier: verifier.expose(),
            state: &code.state,
            expires_in: scope.lifetime().map(|lifetime| lifetime.as_secs()),
        };
        let now = Utc::now();
        let mut tokens = self
            .grant(
                &request,
                &[code.value.expose(), verifier.expose(), &code.state],
            )
            .await?
            .tokens(now)?;
        if tokens.scope.is_empty() {
            tokens.scope = scope.parameter();
        }
        Ok(tokens)
    }

    /// Renew `tokens` with their refresh token, which must be there.
    ///
    /// What the renewal leaves out, the refresh token, the scope and the
    /// subscription, is kept from `tokens`.
    ///
    /// # Errors
    ///
    /// [`LoginError::Malformed`] before any request when `tokens` hold no
    /// refresh token, and otherwise as [`Client::exchange`] fails. None
    /// repeats the refresh token.
    pub async fn refresh(&self, tokens: &Tokens) -> Result<Tokens, LoginError> {
        let refresh = tokens
            .refresh
            .as_ref()
            .ok_or(LoginError::Malformed(UNRENEWABLE))?;
        let request = Refresh {
            grant_type: REFRESH_TOKEN,
            refresh_token: refresh.expose(),
            client_id: self.endpoints.client_id(),
            scope: &tokens.scope,
        };
        let now = Utc::now();
        let mut renewed = self
            .grant(&request, &[refresh.expose()])
            .await?
            .tokens(now)?;
        renewed.refresh = renewed.refresh.or_else(|| tokens.refresh.clone());
        if renewed.scope.is_empty() {
            renewed.scope.clone_from(&tokens.scope);
        }
        renewed.subscription = renewed.subscription.or_else(|| tokens.subscription.clone());
        Ok(renewed)
    }

    async fn grant(&self, request: &impl Serialize, sent: &[&str]) -> Result<Granted, LoginError> {
        let mut secrets = sent.to_vec();
        secrets.extend(self.proxy_password.as_ref().map(SecretValue::expose));
        let sent = secrets.as_slice();
        let response = self
            .http
            .post(self.endpoints.token().clone())
            .header(ACCEPT, JSON)
            .json(request)
            .send()
            .await
            .map_err(|error| transport(error, sent))?;
        let status = response.status();
        let body = read_capped(response, BODY_LIMIT).await;
        if !status.is_success() {
            let refusal = body
                .ok()
                .and_then(|body| serde_json::from_slice::<Failure>(&body).ok())
                .and_then(Failure::refusal);
            return Err(LoginError::Rejected {
                status: status.as_u16(),
                refusal,
            });
        }
        let body = match body {
            Ok(body) => body,
            Err(abnegate_http::Error::UnreadableBody(error)) => return Err(transport(error, sent)),
            Err(_) => return Err(LoginError::Malformed(UNREADABLE)),
        };
        let body = SecretValue::new(
            String::from_utf8(body).map_err(|_| LoginError::Malformed(UNREADABLE))?,
        );
        serde_json::from_str(body.expose()).map_err(|_| LoginError::Malformed(UNREADABLE))
    }
}

fn http(
    proxy: Option<(&Url, Option<(&str, &SecretValue)>)>,
) -> Result<reqwest::Client, LoginError> {
    let builder = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .no_proxy()
        .redirect(Policy::none())
        .dns_resolver(MetadataResolver::new())
        .user_agent(USER_AGENT);
    let builder = match proxy {
        Some((proxy, credentials)) => {
            let proxy = Proxy::all(proxy.clone()).map_err(|error| transport(error, &[]))?;
            builder.proxy(match credentials {
                Some((username, password)) => proxy.basic_auth(username, password.expose()),
                None => proxy,
            })
        }
        None => builder,
    };
    builder.build().map_err(|error| transport(error, &[]))
}

fn transport(error: reqwest::Error, sent: &[&str]) -> LoginError {
    let error = error.without_url();
    let causes = std::iter::successors(
        Some(&error as &(dyn std::error::Error + 'static)),
        |cause| cause.source(),
    )
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join(": ");
    scrubbed(causes, sent)
}

fn scrubbed(text: String, sent: &[&str]) -> LoginError {
    let text = sent
        .iter()
        .filter(|secret| !secret.is_empty())
        .fold(text, |text, secret| text.replace(secret, REDACTED));
    LoginError::Transport(redact(&text).into_owned())
}

#[cfg(test)]
mod tests;
