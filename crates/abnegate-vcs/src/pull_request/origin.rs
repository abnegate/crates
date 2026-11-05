use crate::pull_request::PullRequestError;
use crate::pull_request::PullRequestResult;
use url::Url;

/// The scheme a configured origin uses.
const HTTPS: &str = "https";

/// The scheme an origin may use only when the operator opted in to it.
const HTTP: &str = "http";

/// The label GitHub's own API host and a subdomain-isolated Enterprise one
/// carry ahead of the host their repositories live on.
const API_LABEL: &str = "api.";

/// The path an Enterprise install without subdomain isolation serves REST
/// under.
const VERSIONED_REST: &str = "/api/v3";

/// The segment GraphQL answers at, beside REST or in place of its version.
const GRAPHQL: &str = "graphql";

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
    url: Url,
    host: String,
    plain: bool,
}

impl Origin {
    /// The origin an operator configured: an HTTPS URL with no credentials,
    /// query or fragment, or a plain HTTP one where `permit_http` says the
    /// operator opted in to it. Only an origin that is itself plain HTTP
    /// is reached over it and admits `http` links; an HTTPS one is the same
    /// whether or not plain HTTP was permitted. It answers for its own host,
    /// less the `api.` label carried by GitHub's own API host and by a
    /// subdomain-isolated Enterprise one; an Enterprise install without
    /// subdomain isolation answers for itself under a `/api/v3` path.
    pub(super) fn configured(url: &str, permit_http: bool) -> PullRequestResult<Self> {
        let url = Self::parse(url)?;
        let plain = url.scheme() == HTTP;
        if url.scheme() != HTTPS && !(permit_http && plain) {
            return Err(PullRequestError::InvalidOrigin);
        }
        let host = url
            .host_str()
            .ok_or(PullRequestError::InvalidOrigin)?
            .to_ascii_lowercase();
        let host = host.strip_prefix(API_LABEL).unwrap_or(&host).to_string();
        Ok(Self { url, host, plain })
    }

    /// `url`, over whatever scheme it names, standing in for `host`.
    #[cfg(any(test, feature = "testing"))]
    pub(super) fn standing_in_for(host: &str, url: &str) -> PullRequestResult<Self> {
        Ok(Self {
            url: Self::parse(url)?,
            host: host.to_ascii_lowercase(),
            plain: false,
        })
    }

    /// Whether the origin is plain HTTP, which only an operator's opt-in
    /// admits.
    pub(super) fn plain(&self) -> bool {
        self.plain
    }

    /// Whether a link on this origin's host may use `scheme`: HTTPS always,
    /// and plain HTTP only when the origin itself is plain HTTP.
    pub(super) fn admits(&self, scheme: &str) -> bool {
        scheme.eq_ignore_ascii_case(HTTPS) || (self.plain && scheme.eq_ignore_ascii_case(HTTP))
    }

    fn parse(url: &str) -> PullRequestResult<Url> {
        let url = Url::parse(url).map_err(|_| PullRequestError::InvalidOrigin)?;
        if url.cannot_be_a_base()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(PullRequestError::InvalidOrigin);
        }
        Ok(url)
    }

    /// Whether a repository on `host` is one this origin can be asked about.
    pub(super) fn answers_for(&self, host: &str) -> bool {
        !self.host.is_empty() && host.eq_ignore_ascii_case(&self.host)
    }

    /// Whether `host` is the one this origin's API itself is reached at, such
    /// as `api.github.com`, which may differ from the host it answers for.
    pub(super) fn addressed_at(&self, host: &str) -> bool {
        self.url
            .host_str()
            .is_some_and(|addressed| host.eq_ignore_ascii_case(addressed))
    }

    /// The origin's URL with `segments` appended, each one percent-encoded as
    /// a single path segment, and never an empty segment where the configured
    /// URL ended in a slash.
    pub(super) fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.url.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(segments);
        }
        url
    }

    /// Where GitHub's GraphQL API answers for this origin: beside REST on
    /// GitHub and on an `api.` host, and at `api/graphql` on an Enterprise
    /// install that serves REST under `api/v3`.
    pub(super) fn graphql(&self) -> Url {
        let mut url = self.url.clone();
        let path = url.path();
        let versioned = path
            .strip_suffix('/')
            .unwrap_or(path)
            .ends_with(VERSIONED_REST);
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty();
            if versioned {
                path.pop();
            }
            path.push(GRAPHQL);
        }
        url
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_must_be_https_with_nothing_but_a_host_and_a_path() {
        for refused in [
            "http://api.github.com",
            "api.github.com",
            "",
            "https://user:token@api.github.com",
            "https://api.github.com/?token=x",
            "https://api.github.com/#x",
            "file:///etc/passwd",
            "mailto:someone@github.com",
        ] {
            assert!(Origin::configured(refused, false).is_err(), "{refused}");
        }
    }

    #[test]
    fn plain_http_is_an_origin_only_once_the_operator_opts_in() {
        let refused = Origin::configured("http://localhost:3000", false);
        assert!(
            matches!(refused, Err(PullRequestError::InvalidOrigin)),
            "{refused:?}"
        );

        let admitted = Origin::configured("http://localhost:3000", true).unwrap();
        assert!(admitted.plain());
        assert!(admitted.answers_for("localhost"));
        assert!(admitted.admits("http") && admitted.admits("https"));
        assert_eq!(
            admitted.endpoint(&["repos", "acme", "project"]).as_str(),
            "http://localhost:3000/repos/acme/project"
        );

        for permit_http in [true, false] {
            let secure = Origin::configured("https://api.github.com", permit_http).unwrap();
            assert!(!secure.plain(), "{permit_http}");
            assert!(secure.admits("https"), "{permit_http}");
            assert!(!secure.admits("http"), "{permit_http}");
        }
    }

    #[test]
    fn opting_in_to_plain_http_admits_no_other_scheme_or_shape() {
        for refused in [
            "ftp://github.example.com",
            "file:///etc/passwd",
            "http://user:token@localhost:3000",
            "http://localhost:3000/?token=x",
            "http://localhost:3000/#x",
            "localhost:3000",
        ] {
            assert!(Origin::configured(refused, true).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_trailing_slash_opens_no_empty_segment() {
        for configured in [
            "https://github.example.com/api/v3",
            "https://github.example.com/api/v3/",
        ] {
            assert_eq!(
                Origin::configured(configured, false)
                    .unwrap()
                    .endpoint(&["repos", "acme", "project"])
                    .as_str(),
                "https://github.example.com/api/v3/repos/acme/project"
            );
        }
        assert_eq!(
            Origin::configured("https://api.github.com/", false)
                .unwrap()
                .endpoint(&["repos", "acme", "project"])
                .as_str(),
            "https://api.github.com/repos/acme/project"
        );
    }

    #[test]
    fn graphql_lives_beside_rest_on_github_and_under_api_on_enterprise() {
        for (configured, graphql) in [
            ("https://api.github.com", "https://api.github.com/graphql"),
            ("https://api.github.com/", "https://api.github.com/graphql"),
            (
                "https://api.github.example.com",
                "https://api.github.example.com/graphql",
            ),
            (
                "https://github.example.com/api/v3",
                "https://github.example.com/api/graphql",
            ),
            (
                "https://github.example.com/api/v3/",
                "https://github.example.com/api/graphql",
            ),
        ] {
            assert_eq!(
                Origin::configured(configured, false)
                    .unwrap()
                    .graphql()
                    .as_str(),
                graphql,
                "{configured}"
            );
        }
    }
}
