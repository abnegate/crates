/// The API origin this service addresses, and the repository host it answers for.
///
/// The two are decided together because every request is
/// `{origin}/repos/{owner}/{repository}/...`: the owner and repository come from
/// a repository URL, and the origin they are interpolated into has to be the one
/// that answers for that URL's host. Deciding them apart is what let a
/// `github.com` repository be accepted while its request -- and the token sent
/// with it -- went to a configured Enterprise origin.
#[derive(Debug, Clone)]
pub(super) struct Origin {
    pub(super) url: String,
    pub(super) host: String,
}

impl Origin {
    /// The origin an operator configured. It answers for its own host, less the
    /// `api.` label carried by GitHub's own API host and by a subdomain-isolated
    /// Enterprise one; an Enterprise install without subdomain isolation answers
    /// for itself under a `/api/v3` path.
    pub(super) fn configured(url: String) -> Self {
        let authority = url
            .split_once("://")
            .map_or(url.as_str(), |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default();
        let host = host_of(authority).to_ascii_lowercase();
        let host = host.strip_prefix("api.").unwrap_or(&host).to_string();
        Self { url, host }
    }

    pub(super) fn standing_in_for(host: &str, url: String) -> Self {
        Self {
            url,
            host: host.to_ascii_lowercase(),
        }
    }

    /// Whether a repository on `host` is one this origin can be asked about.
    pub(super) fn answers_for(&self, host: &str) -> bool {
        !self.host.is_empty() && host.eq_ignore_ascii_case(&self.host)
    }
}

/// The host of an authority, without userinfo or port.
pub(super) fn host_of(authority: &str) -> &str {
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    match host.strip_prefix('[') {
        Some(tail) => tail.split_once(']').map_or(host, |(inside, _)| inside),
        None => host.split_once(':').map_or(host, |(host, _)| host),
    }
}
