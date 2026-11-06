mod exchange;
mod failure;
mod granted;
mod reason;
mod refresh;

use std::time::Duration;

use abnegate_http::metadata::MetadataResolver;
use abnegate_http::read_capped;
use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use abnegate_secret::redact;
use chrono::Utc;
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

const TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = concat!("abnegate-agent-cli/", env!("CARGO_PKG_VERSION"));
const BODY_LIMIT: usize = 64 * 1024;
const JSON: &str = "application/json";
const AUTHORIZATION_CODE: &str = "authorization_code";
const REFRESH_TOKEN: &str = "refresh_token";
const UNREADABLE: &str = "Claude's token endpoint sent a grant that could not be read";
const UNRENEWABLE: &str = "This Claude sign-in has no refresh token";

/// Exchanges a sign-in's code for [`Tokens`], and renews them, at the
/// [`Endpoints::token`] endpoint.
///
/// Its HTTP client is its own: it goes through no proxy, follows no
/// redirect, so a redirect is a [`LoginError::Rejected`], drops link-local
/// and cloud metadata addresses a name resolves to, gives up on a request
/// after 30 seconds, reads at most 64 KiB of an answer, and names itself
/// `abnegate-agent-cli/<version>`.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoints: Endpoints,
}

impl Client {
    /// A client for `endpoints`. It fails only when the HTTP client cannot
    /// be built, such as when no TLS backend can start.
    pub fn new(endpoints: Endpoints) -> Result<Self, LoginError> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .no_proxy()
            .redirect(Policy::none())
            .dns_resolver(MetadataResolver::new())
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| transport(error, &[]))?;
        Ok(Self { http, endpoints })
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
            expires_in: scope.lifetime(),
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
